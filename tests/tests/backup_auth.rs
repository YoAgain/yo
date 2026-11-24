//! Who may fetch a backup record (spec 7): only a request signed by one of its doors' auth keys, fresh and once; any
//! other answer is the `404` of no record, also for a record without the keys (from before them), which the app makes
//! anew. What K and the storage key seal lands on the sizes the hub may see.

use libsignal_protocol::PrivateKey;
use yo_core::backup::{self, AccountEntropyPool, Door, PasskeyDoor};
use yo_core::{Backup, Client, Push};
use yo_hub::db::{BackupAuth, Db};
use yo_tests::{random_u64, Http, TestHub};
use yo_wire::api;
use yo_wire::limits::SEALED_SIZES;
use yo_wire::request::{to_be_signed, HEADER_SIGNATURE, HEADER_TIME};

fn register(hub: &TestHub) -> Client {
    let name = format!("Safe{}", random_u64() % 1_000_000);
    Client::register_username(Http::new(hub.url()), &name, None, None, Push::none()).unwrap()
}

/// A GET of the record by `id` as anyone could send it: signed with `key` at `time` (none: unsigned). The status.
fn get(hub: &TestHub, id: &[u8], signed: Option<(&PrivateKey, u64)>) -> (u16, Vec<u8>) {
    let path = api::backup_path(id);
    get_as(hub, id, signed.map(|(key, time)| (sign(key, &to_be_signed("GET", &path, time, b"")), time)))
}

/// `get` with a signature as given (XEdDSA signs each time anew: a replay sends the same one again).
fn get_as(hub: &TestHub, id: &[u8], signed: Option<(Vec<u8>, u64)>) -> (u16, Vec<u8>) {
    let path = api::backup_path(id);
    let mut request = reqwest::blocking::Client::new().get(format!("{}{path}", hub.url()));
    if let Some((signature, time)) = signed {
        request = request.header(HEADER_TIME, time.to_string()).header(HEADER_SIGNATURE, yo_wire::b64(&signature));
    }
    let response = request.send().unwrap();
    (response.status().as_u16(), response.bytes().unwrap().to_vec())
}

/// A door's XEdDSA signature (spec 7).
fn sign(key: &PrivateKey, message: &[u8]) -> Vec<u8> {
    key.calculate_signature(message, &mut rand::rng()).unwrap().to_vec()
}

fn words(seed: u8) -> AccountEntropyPool {
    yo_tests::backup::pool(&yo_tests::backup::recovery_key([seed; 16]))
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
    assert_eq!(get(&hub, &user, Some((&PrivateKey::deserialize(&[4; 32]).unwrap(), now))), not_found);
    assert_eq!(get(&hub, &user, Some((&Door::Code(&words(5)).auth_key(), now))), not_found);
    // The same as for no record at all, also signed.
    assert_eq!(get(&hub, &[6; 16], Some((&by_code, now))), not_found);
    // Either door's key, by the user id or by the locator.
    let body = yo_wire::encode(&record);
    let first = sign(&by_code, &to_be_signed("GET", &api::backup_path(&user), now, b""));
    assert_eq!(get_as(&hub, &user, Some((first.clone(), now))), (200, body.clone()));
    assert_eq!(get(&hub, &locator, Some((&by_code, now + 1))), (200, body.clone()));
    assert_eq!(get(&hub, &user, Some((&by_passkey, now + 2))), (200, body.clone()));
    // Each signature once; none out of the window; none for another path.
    assert_eq!(get_as(&hub, &user, Some((first, now))), not_found, "replayed");
    let stale = now - yo_wire::limits::MAX_SKEW_MS - 1000;
    assert_eq!(get(&hub, &user, Some((&by_code, stale))), not_found, "stale");
    assert_eq!(get(&hub, &user, Some((&by_code, now + yo_wire::limits::MAX_SKEW_MS + 60_000))), not_found, "ahead");
    let path = api::backup_path(&locator);
    let signature = sign(&by_code, &to_be_signed("GET", &path, now + 3, b""));
    let wrong_path = reqwest::blocking::Client::new()
        .get(format!("{}{}", hub.url(), api::backup_path(&user)))
        .header(HEADER_TIME, (now + 3).to_string())
        .header(HEADER_SIGNATURE, yo_wire::b64(&signature))
        .send()
        .unwrap();
    assert_eq!(wrong_path.status().as_u16(), 404, "signed for the locator, sent for the user id");
    // The core signs: by the recovery key, by the passkey; a door the record lacks finds nothing.
    let http = Http::new(hub.url());
    assert_eq!(Client::fetch_backup(&*http, &user, &Door::Passkey(&prf), &[]), Ok(Some(record.clone())));
    let without = backup::make(&k, b"{}", &code, PasskeyDoor::None).unwrap();
    t.put_backup(&without).unwrap();
    assert_eq!(Client::fetch_backup(&*http, &user, &Door::Passkey(&prf), &[]), Ok(None));
    assert_eq!(*Client::open_backup(&*http, None, &Door::Code(&code), &[]).unwrap().unwrap().k, k);
    // A record whose auth key is no canonical Curve25519 key is refused.
    let bad = Backup { auth_code: vec![0xff; 32].into(), ..without };
    assert_eq!(t.signed_for_tests("PUT", api::BACKUP, &yo_wire::encode(&bad)).unwrap().status, 400);
}

/// A hub on a database file, and that file opened beside it: the test's way to store a record as an older hub did,
/// without the auth keys.
fn hub_on_file() -> (TestHub, Db) {
    let path = std::env::temp_dir().join(format!("yo-keyless-{}-{}.sqlite", std::process::id(), random_u64()));
    let mut config = yo_hub::config::Config::for_tests();
    config.db_path = path.to_str().unwrap().to_owned();
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let db = Db::open(path.to_str().unwrap()).unwrap();
    (hub, db)
}

#[test]
fn a_record_without_auth_keys_is_never_served_and_the_app_makes_a_new_one() {
    let (old_k, k, prf, code) = ([1; 32], [4; 32], [2; 32], words(7));
    let (hub, db) = hub_on_file();
    let mut t = register(&hub);
    let (user, secret) = (yo_tests::user_id(t.user_id()), t.account_secret());
    let old = yo_wire::encode(&backup::make(&old_k, b"{}", &code, PasskeyDoor::New(&prf)).unwrap());
    db.put_backup(&t.device_id(), Some(&backup::locator(&code)), &old, &BackupAuth::default(), 0).unwrap();
    // To no one: unsigned, or signed by either door.
    let http = Http::new(hub.url());
    assert_eq!(get(&hub, &user, None).0, 404);
    assert_eq!(Client::open_backup(&*http, None, &Door::Code(&code), &[]), Ok(None));
    assert_eq!(Client::open_backup(&*http, Some(&user), &Door::Passkey(&prf), &[]), Ok(None));
    // The app makes a new one from the recovery key on the phone: a new K, the recovery key's door only.
    t.put_backup(&backup::make(&k, b"{\"new\":1}", &code, PasskeyDoor::None).unwrap()).unwrap();
    let opened = Client::open_backup(&*http, None, &Door::Code(&code), &[]).unwrap().unwrap();
    assert_eq!((*opened.k, opened.plain.as_slice()), (k, &b"{\"new\":1}"[..]));
    assert_eq!(Client::open_backup(&*http, Some(&user), &Door::Passkey(&prf), &[]), Ok(None), "no passkey yet");
    // Confirmed: the passkey asked once, its door added to the record as it is (the same K).
    let record = backup::make(&opened.k, &opened.plain, &code, PasskeyDoor::New(&prf)).unwrap();
    t.put_backup(&record).unwrap();
    // A new phone, by the passkey: the record, K, and the account registered again with its key.
    let by_passkey = Client::open_backup(&*http, Some(&user), &Door::Passkey(&prf), &[]).unwrap().unwrap();
    assert_eq!(*by_passkey.k, k);
    let name = t.username().unwrap().to_owned();
    let new = Client::register_username(Http::new(hub.url()), &name, Some(secret), None, Push::none()).unwrap();
    assert_eq!(new.user_id(), t.user_id());
    assert!(Client::open_backup(&*http, None, &Door::Code(&code), &[]).unwrap().is_some(), "and by the recovery key");
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
    for (round, count) in [1, 10, 40, 160].into_iter().enumerate() {
        for i in 0..count {
            t.set_friend(&format!("{:032x}", round * 1000 + i), Some(format!("Friend number {i}")), Some(1)).unwrap();
        }
        t.maintenance().unwrap();
        let response = t.signed_for_tests("GET", api::STORAGE, &[]).unwrap();
        let stored: api::Storage = yo_wire::decode(&response.body).unwrap();
        assert!(SEALED_SIZES.contains(&stored.data.as_slice().len()), "{}", stored.data.as_slice().len());
        seen.insert(stored.data.as_slice().len());
    }
    assert_eq!(seen, SEALED_SIZES.into_iter().collect(), "grew through every bucket");
}

/// The same XEdDSA signature written another way: its scalar plus the group order, where that still fits the
/// signature's 253 bits (most do). An accepting verifier would take it as a new signature.
fn other_encoding(signature: &[u8]) -> Option<Vec<u8>> {
    const ORDER: [u8; 32] = [
        0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
    ];
    let mut s: [u8; 32] = signature[32..].try_into().unwrap();
    let sign = s[31] & 0x80;
    s[31] &= 0x7f;
    let mut carry = 0u16;
    for (byte, add) in s.iter_mut().zip(ORDER) {
        let sum = *byte as u16 + add as u16 + carry;
        *byte = sum as u8;
        carry = sum >> 8;
    }
    (s[31] & 0xe0 == 0).then(|| [&signature[..32], &s[..31], &[s[31] | sign]].concat())
}

#[test]
fn a_signature_written_another_way_is_still_a_replay() {
    let hub = TestHub::start();
    let mut t = register(&hub);
    let user = yo_tests::user_id(t.user_id());
    let code = words(9);
    t.put_backup(&backup::make(&[1; 32], b"{}", &code, PasskeyDoor::None).unwrap()).unwrap();
    let key = Door::Code(&code).auth_key();
    let now = yo_hub::now_ms();
    let (first, other) = (0..64)
        .find_map(|n| {
            let first = sign(&key, &to_be_signed("GET", &api::backup_path(&user), now + n, b""));
            other_encoding(&first).map(|other| ((first, now + n), (other, now + n)))
        })
        .unwrap();
    assert_eq!(get_as(&hub, &user, Some(first)).0, 200);
    assert_eq!(get_as(&hub, &user, Some(other)), (404, Vec::new()), "the same signature, its scalar not reduced");
}

/// What a party between the app and the hub (a TLS proxy) would do: keep an answer and hand it back later.
struct Replaying {
    http: std::sync::Arc<Http>,
    kept: std::sync::Mutex<Option<Vec<u8>>>,
    replay: std::sync::atomic::AtomicBool,
}

impl yo_core::Transport for Replaying {
    fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<yo_core::Response, String> {
        let mut response = self.http.request(method, path, headers, body)?;
        let mut kept = self.kept.lock().unwrap();
        if self.replay.load(std::sync::atomic::Ordering::Relaxed) {
            response.body = kept.clone().expect("an answer kept");
        } else if response.status == 200 {
            *kept = Some(response.body.clone());
        }
        Ok(response)
    }
}

/// B2 (rollback): with the hub's key pinned, a backup's fetch asks with a nonce and takes only the hub's answer to
/// it (spec 6.10): an older record kept by a party between (made with a recovery key since replaced) is refused,
/// though it would open with that old key. An app without pinned keys, or an older hub's unsigned answer, as before.
#[test]
fn a_pinned_fetch_takes_only_the_hubs_answer_to_it() {
    let hub = TestHub::start();
    let mut t = register(&hub);
    let user = yo_tests::user_id(t.user_id());
    let (k, prf, old_code) = ([1; 32], [2; 32], words(11));
    t.put_backup(&backup::make(&k, b"{\"old\":1}", &old_code, PasskeyDoor::New(&prf)).unwrap()).unwrap();
    let pinned = [yo_tests::hub_key()];
    let between = Replaying { http: Http::new(hub.url()), kept: Default::default(), replay: Default::default() };
    // Signed and fresh: taken, by either door.
    let old = Client::open_backup(&between, None, &Door::Code(&old_code), &pinned).unwrap().unwrap();
    assert_eq!(old.plain, b"{\"old\":1}");
    assert!(Client::open_backup(&between, Some(&user), &Door::Passkey(&prf), &pinned).unwrap().is_some());
    // A new recovery key and a new K: the record the hub has now.
    let new_code = words(12);
    t.put_backup(&backup::make(&[3; 32], b"{\"new\":1}", &new_code, PasskeyDoor::New(&prf)).unwrap()).unwrap();
    between.replay.store(true, std::sync::atomic::Ordering::Relaxed);
    // The old answer handed back for a new request: its signature is for another nonce.
    for door in [Door::Passkey(&prf), Door::Code(&new_code)] {
        assert_eq!(
            Client::open_backup(&between, Some(&user), &door, &pinned).map(|o| o.map(|o| o.plain)),
            Err(yo_core::Error::Protocol("answer not signed by the hub")),
        );
    }
    // Its signature cut off, unpinned (a hub not listed), it would have been taken: what the pin is for.
    let signed = between.kept.lock().unwrap().clone().unwrap();
    *between.kept.lock().unwrap() = Some(signed[..signed.len() - 64].to_vec());
    let taken = Client::open_backup(&between, Some(&user), &Door::Passkey(&prf), &[]).unwrap().unwrap();
    assert_eq!(taken.plain, b"{\"old\":1}", "the old record, opened by the passkey's door it kept");
    // The answer stripped of its signature, or signed by another key: refused.
    let plain = yo_wire::encode(&backup::make(&[3; 32], b"{}", &new_code, PasskeyDoor::None).unwrap());
    *between.kept.lock().unwrap() = Some(plain);
    assert!(Client::fetch_backup(&between, &user, &Door::Code(&new_code), &pinned).is_err(), "unsigned");
    let stranger = libsignal_protocol::KeyPair::generate(&mut rand::rng());
    let other: [u8; 32] = stranger.public_key.public_key_bytes().try_into().unwrap();
    between.replay.store(false, std::sync::atomic::Ordering::Relaxed);
    assert!(Client::fetch_backup(&between, &user, &Door::Code(&new_code), &[other]).is_err(), "another hub's key");
    // The hub's key among several pinned (the spare, 6.10): taken, the new record.
    let fresh = Client::open_backup(&between, None, &Door::Code(&new_code), &[other, pinned[0]]).unwrap().unwrap();
    assert_eq!((*fresh.k, fresh.plain.as_slice()), ([3; 32], &b"{\"new\":1}"[..]));
    // Without a nonce the hub answers unsigned, as before (an app from before; the tools).
    let plain = Client::fetch_backup(&*Http::new(hub.url()), &user, &Door::Code(&new_code), &[]).unwrap().unwrap();
    assert_eq!(open_plain(plain, &new_code), b"{\"new\":1}");
}

fn open_plain(record: Backup, code: &AccountEntropyPool) -> Vec<u8> {
    backup::open(record, &Door::Code(code)).unwrap().plain
}

/// B2 at the hub: a fetch asked with `Yo-Nonce` is answered with the record and the hub's signature over the nonce,
/// the path and the record (spec 6.10); without the header, the record alone, as before.
#[test]
fn the_hub_signs_a_fetch_asked_with_a_nonce() {
    let hub = TestHub::start();
    let mut t = register(&hub);
    let user = yo_tests::user_id(t.user_id());
    let code = words(13);
    let record = yo_wire::encode(&backup::make(&[5; 32], b"{}", &code, PasskeyDoor::None).unwrap());
    t.put_backup(&yo_wire::decode(&record).unwrap()).unwrap();
    let path = api::backup_path(&user);
    let fetch = |nonce: Option<[u8; 16]>, time: u64| {
        let signature = sign(&Door::Code(&code).auth_key(), &to_be_signed("GET", &path, time, b""));
        let mut request = reqwest::blocking::Client::new()
            .get(format!("{}{path}", hub.url()))
            .header(HEADER_TIME, time.to_string())
            .header(HEADER_SIGNATURE, yo_wire::b64(&signature));
        if let Some(nonce) = nonce {
            request = request.header(yo_wire::hubsig::NONCE_HEADER, yo_wire::b64(&nonce));
        }
        let response = request.send().unwrap();
        (response.status().as_u16(), response.bytes().unwrap().to_vec())
    };
    let now = yo_hub::now_ms();
    assert_eq!(fetch(None, now), (200, record.clone()), "no nonce: as before");
    let (status, signed) = fetch(Some([7; 16]), now + 1);
    assert_eq!(status, 200);
    let key = yo_tests::hub_key();
    assert_eq!(yo_wire::hubsig::open(&key, &[7; 16], &path, b"", &signed), Ok(record.as_slice()));
    assert!(yo_wire::hubsig::open(&key, &[8; 16], &path, b"", &signed).is_err(), "for its nonce only");
}
