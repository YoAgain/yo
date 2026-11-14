//! Who may fetch a backup record (spec 7): only a request signed by one of its doors' auth keys, fresh and once; any
//! other answer is the `404` of no record. Records from before the auth keys are handed out without a signature until
//! the cutoff, to no one after, and the core rewrites them with the keys. What K and the storage key seal lands on
//! the sizes the hub may see.

use ed25519_dalek::{Signer as _, SigningKey};
use yo_core::backup::{self, Door, PasskeyDoor};
use yo_core::{Backup, Client, Push};
use yo_hub::db::{BackupAuth, Db};
use yo_tests::{random_u64, Http, TestHub};
use yo_wire::api::{self, LegacyBackup};
use yo_wire::limits::SEALED_SIZES;
use yo_wire::request::{to_be_signed, HEADER_SIGNATURE, HEADER_TIME};

fn register(hub: &TestHub) -> Client {
    let name = format!("Safe{}", random_u64() % 1_000_000);
    Client::register_username(Http::new(hub.url()), &name, None, None, Push::none()).unwrap()
}

/// A GET of the record by `id` as anyone could send it: signed with `key` at `time` (none: unsigned). The status.
fn get(hub: &TestHub, id: &[u8], signed: Option<(&SigningKey, u64)>) -> (u16, Vec<u8>) {
    let path = api::backup_path(id);
    let mut request = reqwest::blocking::Client::new().get(format!("{}{path}", hub.url()));
    if let Some((key, time)) = signed {
        let signature = key.sign(&to_be_signed("GET", &path, time, b""));
        request =
            request.header(HEADER_TIME, time.to_string()).header(HEADER_SIGNATURE, yo_wire::b64(&signature.to_bytes()));
    }
    let response = request.send().unwrap();
    (response.status().as_u16(), response.bytes().unwrap().to_vec())
}

fn words(seed: u8) -> Vec<String> {
    yo_tests::backup::words([seed; 16])
}

#[test]
fn only_a_door_fetches_a_record() {
    let hub = TestHub::start();
    let mut t = register(&hub);
    let user = yo_tests::user_id(t.user_id());
    let (k, prf, code) = ([1; 32], [2; 32], words(3));
    let record = backup::make(&k, b"{}", &code, PasskeyDoor::New(&prf)).unwrap();
    t.put_backup(&record).unwrap();
    let now = yo_hub::now_ms();
    let (by_code, by_passkey) = (Door::Code(&code).auth_key(), Door::Passkey(&prf).auth_key());
    let not_found = (404, Vec::new());
    let locator = backup::locator(&code);
    // No signature, a stranger's key, or the device's own: as if there were no record.
    assert_eq!(get(&hub, &user, None), not_found);
    assert_eq!(get(&hub, &locator, None), not_found);
    assert_eq!(get(&hub, &user, Some((&SigningKey::from_bytes(&[4; 32]), now))), not_found);
    assert_eq!(get(&hub, &user, Some((&Door::Code(&words(5)).auth_key(), now))), not_found);
    // The same as for no record at all, also signed.
    assert_eq!(get(&hub, &[6; 16], Some((&by_code, now))), not_found);
    // Either door's key, by the user id or by the locator.
    let body = yo_wire::encode(&record);
    assert_eq!(get(&hub, &user, Some((&by_code, now))), (200, body.clone()));
    assert_eq!(get(&hub, &locator, Some((&by_code, now + 1))), (200, body.clone()));
    assert_eq!(get(&hub, &user, Some((&by_passkey, now + 2))), (200, body.clone()));
    // Each signature once; none out of the window; none for another path.
    assert_eq!(get(&hub, &user, Some((&by_code, now))), not_found, "replayed");
    let stale = now - yo_wire::limits::MAX_SKEW_MS - 1000;
    assert_eq!(get(&hub, &user, Some((&by_code, stale))), not_found, "stale");
    assert_eq!(get(&hub, &user, Some((&by_code, now + yo_wire::limits::MAX_SKEW_MS + 60_000))), not_found, "ahead");
    let path = api::backup_path(&locator);
    let signature = by_code.sign(&to_be_signed("GET", &path, now + 3, b"")).to_bytes();
    let wrong_path = reqwest::blocking::Client::new()
        .get(format!("{}{}", hub.url(), api::backup_path(&user)))
        .header(HEADER_TIME, (now + 3).to_string())
        .header(HEADER_SIGNATURE, yo_wire::b64(&signature))
        .send()
        .unwrap();
    assert_eq!(wrong_path.status().as_u16(), 404, "signed for the locator, sent for the user id");
    // The core signs: by the words, by the passkey; a door the record lacks finds nothing.
    let http = Http::new(hub.url());
    assert_eq!(Client::fetch_backup(&*http, &user, &Door::Passkey(&prf)), Ok(Some(record.clone())));
    let without = backup::make(&k, b"{}", &code, PasskeyDoor::None).unwrap();
    t.put_backup(&without).unwrap();
    assert_eq!(Client::fetch_backup(&*http, &user, &Door::Passkey(&prf)), Ok(None));
    assert_eq!(Client::open_backup(&*http, None, &Door::Code(&code)).unwrap().unwrap().k, k);
    // A record whose auth key is no Ed25519 point is refused.
    let bad = Backup { auth_code: vec![2; 32].into(), ..without };
    assert_eq!(t.signed_for_tests("PUT", api::BACKUP, &yo_wire::encode(&bad)).unwrap().status, 400);
}

/// A hub on a database file, and that file opened beside it: the test's way to store a record as an older hub did.
fn hub_on_file(legacy_until: u64) -> (TestHub, Db) {
    let path = std::env::temp_dir().join(format!("yo-legacy-{}-{}.sqlite", std::process::id(), random_u64()));
    let mut config = yo_hub::config::Config::for_tests();
    config.db_path = path.to_str().unwrap().to_owned();
    config.limits.legacy_backups_until_ms = legacy_until;
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let db = Db::open(path.to_str().unwrap()).unwrap();
    (hub, db)
}

/// A record as the app made it before the auth keys: version 1, the sealed part not padded.
fn legacy(k: &[u8; 32], prf: &[u8], code: &[String]) -> Vec<u8> {
    let made = backup::make(k, b"{}", code, PasskeyDoor::New(prf)).unwrap();
    yo_wire::encode(&LegacyBackup {
        version: 1,
        by_passkey: made.by_passkey,
        by_code: made.by_code,
        sealed: made.sealed,
        locator: made.locator,
    })
}

#[test]
fn a_record_from_before_the_auth_keys_is_served_until_the_cutoff_and_rewritten() {
    let (k, prf, code) = ([1; 32], [2; 32], words(7));
    let (hub, db) = hub_on_file(yo_hub::config::LEGACY_BACKUPS_UNTIL_MS);
    assert!(yo_hub::now_ms() < yo_hub::config::LEGACY_BACKUPS_UNTIL_MS, "the cutoff is ahead");
    let mut t = register(&hub);
    let user = yo_tests::user_id(t.user_id());
    let old = legacy(&k, &prf, &code);
    db.put_backup(&t.device_id(), Some(&backup::locator(&code)), &old, &BackupAuth::default(), 0).unwrap();
    // Before the cutoff: to anyone, unsigned, as before.
    assert_eq!(get(&hub, &user, None), (200, old.clone()));
    // The core reads it, by either door, and rewrites it with the words' auth key; the passkey's wrap is kept.
    let http = Http::new(hub.url());
    let opened = Client::open_backup(&*http, Some(&user), &Door::Passkey(&prf)).unwrap().unwrap();
    assert_eq!((opened.k, opened.record.version), (k, 1));
    let opened = Client::open_backup(&*http, None, &Door::Code(&code)).unwrap().unwrap();
    t.put_backup(&backup::make(&k, &opened.plain, &code, PasskeyDoor::Keep(&opened.record)).unwrap()).unwrap();
    assert_eq!(get(&hub, &user, None).0, 404, "no longer to anyone");
    assert!(Client::open_backup(&*http, None, &Door::Code(&code)).unwrap().is_some());
    // The passkey still opens K, but signs nothing until it is confirmed (a passkey asked once).
    let rewritten = Client::fetch_backup(&*http, &user, &Door::Code(&code)).unwrap().unwrap();
    assert!(rewritten.auth_passkey.as_slice().is_empty() && !rewritten.by_passkey.as_slice().is_empty());
    assert_eq!(Client::open_backup(&*http, Some(&user), &Door::Passkey(&prf)), Ok(None));
    assert!(backup::make(&k, b"{}", &code, PasskeyDoor::Confirm(&rewritten, &[9; 32])).is_err(), "another passkey");
    t.put_backup(&backup::make(&k, b"{}", &code, PasskeyDoor::Confirm(&rewritten, &prf)).unwrap()).unwrap();
    assert_eq!(Client::open_backup(&*http, Some(&user), &Door::Passkey(&prf)).unwrap().unwrap().k, k);

    // After the cutoff: a record without the auth keys goes to no one, signed or not.
    let (hub, db) = hub_on_file(0);
    let t = register(&hub);
    let user = yo_tests::user_id(t.user_id());
    db.put_backup(&t.device_id(), Some(&backup::locator(&code)), &old, &BackupAuth::default(), 0).unwrap();
    assert_eq!(get(&hub, &user, None).0, 404);
    let http = Http::new(hub.url());
    assert_eq!(Client::open_backup(&*http, None, &Door::Code(&code)), Ok(None));
    assert_eq!(Client::open_backup(&*http, Some(&user), &Door::Passkey(&prf)), Ok(None));
}

#[test]
fn what_k_and_the_storage_key_seal_lands_on_the_sizes() {
    let (k, code) = ([1; 32], words(8));
    for (len, size) in [(10, 1024), (2000, 4096), (10_000, 16 * 1024), (40_000, 60 * 1024)] {
        let record = backup::make(&k, &vec![b'x'; len], &code, PasskeyDoor::None).unwrap();
        assert_eq!(record.sealed.as_slice().len(), size);
    }
    assert!(backup::make(&k, &[b'x'; 61_408], &code, PasskeyDoor::None).is_err(), "past the largest");
    // The storage at the hub: friends added until it grows past a bucket, every write one of the sizes.
    let hub = TestHub::start();
    let mut t = register(&hub);
    let mut seen = std::collections::BTreeSet::new();
    for round in 0..3 {
        for i in 0..40 {
            t.set_friend(&format!("{:032x}", round * 1000 + i), Some(format!("Friend number {i}")), Some(1)).unwrap();
        }
        t.maintenance().unwrap();
        let response = t.signed_for_tests("GET", api::STORAGE, &[]).unwrap();
        let stored: api::Storage = yo_wire::decode(&response.body).unwrap();
        assert!(SEALED_SIZES.contains(&stored.data.as_slice().len()), "{}", stored.data.as_slice().len());
        seen.insert(stored.data.as_slice().len());
    }
    assert!(seen.len() > 1, "grew past a bucket: {seen:?}");
}
