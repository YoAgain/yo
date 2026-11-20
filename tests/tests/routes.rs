//! Every route of the hub as spec 6.3 lists it: what it answers to a good request, to a missing or bad signature, to
//! a malformed or oversized body, to a repeat, past its limit; and the prekeys (4.1, 6.5) as the hub hands them out.

use std::sync::Arc;
use yo_core::{Client, Error, Event};
use yo_hub::config::Config;
use yo_tests::{junk, new_phone, random_u64, register, user_id, username, Http, TestHub};
use yo_wire::api::{self, status};
use yo_wire::handle::Handle;
use yo_wire::{decode, encode};

/// A request as anyone can send it: method, path, headers, body; the status and the body of the answer.
fn raw(hub: &TestHub, method: &str, path: &str, headers: &[(&str, String)], body: Vec<u8>) -> (u16, Vec<u8>) {
    let mut request =
        reqwest::blocking::Client::new().request(method.parse().unwrap(), format!("{}{path}", hub.url())).body(body);
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let response = request.send().unwrap();
    (response.status().as_u16(), response.bytes().unwrap().to_vec())
}

/// A request signed by the client's device, whatever its body.
fn signed(client: &mut Client, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let response = client.signed_for_tests(method, path, body).unwrap();
    (response.status, response.body)
}

fn account(hub: &TestHub) -> (Client, Arc<Http>, String) {
    let http = Http::new(hub.url());
    let name = username(random_u64() % 1_000_000_000);
    (register(http.clone(), &name, None), http, name)
}

fn hub_with(limits: impl FnOnce(&mut yo_hub::config::Limits)) -> TestHub {
    let mut config = Config::for_tests();
    limits(&mut config.limits);
    TestHub::start_with(config, "127.0.0.1:0")
}

fn lookup_body(handle: Handle, flags: u8, except: Vec<[u8; 32]>) -> Vec<u8> {
    encode(&api::Lookup { handle, flags, except })
}

/// A lookup of `who` with prekeys, as the hub answers it.
fn bundles(looker: &mut Client, who: &str, except: Vec<[u8; 32]>) -> api::LookupResult {
    let (code, body) = signed(looker, "POST", api::LOOKUP, &lookup_body(Handle::user(&user_id(who)), 1, except));
    assert_eq!(code, status::OK);
    decode(&body).unwrap()
}

/// The device's one-time prekeys at the hub, as its inbox answer tells them.
fn prekeys_left(client: &mut Client) -> u32 {
    let (code, body) = signed(client, "GET", &api::inbox_path(0), &[]);
    assert_eq!(code, status::OK);
    decode::<api::Inbox>(&body).unwrap().prekeys_left
}

fn signed_prekey(id: u32, len: usize) -> api::SignedPrekey {
    api::SignedPrekey { id, key: vec![id as u8; len].into(), signature: vec![7; 64].into() }
}

/// An upload of `n` one-time prekeys of each kind from id `from` on.
fn one_time(from: u32, n: u32) -> api::Prekeys {
    api::Prekeys {
        signed: None,
        last_resort: None,
        one_time: (from..from + n).map(|id| api::Prekey { id, key: vec![5; 33].into() }).collect(),
        kyber: (from..from + n).map(|id| signed_prekey(id, 1569)).collect(),
    }
}

fn backup_record(locator: Vec<u8>) -> api::Backup {
    let auth = yo_tests::account_key(&[9; 32]); // a door's Curve25519 key
    api::Backup::new(Vec::new(), vec![2; 60], vec![3; 1024], locator, None, auth)
}

/// Every signed route with a body it takes: method, path, body.
fn signed_routes(me: &str) -> Vec<(&'static str, String, Vec<u8>)> {
    vec![
        ("PUT", api::PREKEYS.into(), encode(&one_time(1_000_000, 1))),
        ("POST", api::LOOKUP.into(), lookup_body(Handle::user(&user_id(me)), 0, Vec::new())),
        ("PUT", api::USERNAME.into(), encode(&api::SetUsername { username: None })),
        ("PUT", api::PUSH.into(), encode(&api::Push::none())),
        ("GET", api::inbox_path(0), Vec::new()),
        ("POST", api::INBOX_ACK.into(), encode(&api::Ack { ids: vec![1] })),
        ("PUT", api::BACKUP.into(), encode(&backup_record(Vec::new()))),
        ("GET", api::STORAGE.into(), Vec::new()),
        ("PUT", api::STORAGE.into(), encode(&api::Storage { version: 0, data: vec![1; 1024].into() })),
        ("POST", api::REPORT.into(), encode(&api::Report { user_id: [1; 16], reason: api::REPORT_SPAM })),
        (
            "POST",
            api::FEEDBACK.into(),
            encode(&api::Feedback { text: b"hi".to_vec().into(), app_version: b"1".to_vec().into() }),
        ),
        ("GET", api::SUPPORT.into(), Vec::new()),
        ("POST", api::SUPPORT_READ.into(), encode(&api::SupportRead { through: 0 })),
        (
            "POST",
            api::SUPPORT.into(),
            encode(&api::Feedback { text: b"hi".to_vec().into(), app_version: Vec::new().into() }),
        ),
        ("DELETE", api::ACCOUNT.into(), Vec::new()),
    ]
}

/// Spec 6.2: a signed route answers `401` to a request without a signature, with another request's, from a device
/// the hub does not know and to a replay; `410` to a device replaced by a new phone and to one whose account is gone.
#[test]
fn every_signed_route_wants_its_devices_signature() {
    let hub = TestHub::start();
    let (mut a, http, name) = account(&hub);
    let me = a.user_id().to_owned();
    a.sync(0).unwrap();
    // Headers of a request signed for another route: the signature covers method, path, time and body.
    let (_, _, borrowed, _) = http.recorded.lock().unwrap().last().cloned().unwrap();
    let borrowed: Vec<(&str, String)> = borrowed.iter().map(|(n, v)| (n.as_str(), v.clone())).collect();
    let stranger = [
        ("yo-device", yo_wire::b64(&[7; 16])),
        ("yo-time", yo_hub::now_ms().to_string()),
        ("yo-signature", yo_wire::b64(&[1; 64])),
    ];
    for (method, path, body) in signed_routes(&me) {
        for headers in [&[][..], &borrowed[..], &stranger[..]] {
            let (code, _) = raw(&hub, method, &path, headers, body.clone());
            assert_eq!(code, status::UNAUTHORIZED, "{method} {path} {headers:?}");
        }
        // A good signature on a body changed after it.
        let before = http.recorded.lock().unwrap().len();
        signed(&mut a, method, &path, &body);
        let (_, _, headers, _) = http.recorded.lock().unwrap()[before].clone();
        let headers: Vec<(&str, String)> = headers.iter().map(|(n, v)| (n.as_str(), v.clone())).collect();
        let mut changed = body.clone();
        changed.push(0);
        if path == api::ACCOUNT {
            break;
        }
        assert_eq!(raw(&hub, method, &path, &headers, changed).0, status::UNAUTHORIZED, "{method} {path} changed");
        // Sent once already: a replay.
        assert_eq!(raw(&hub, method, &path, &headers, body.clone()).0, status::UNAUTHORIZED, "{method} {path}");
    }
    // Deleted with the last route: every route answers 410 from now on. And a device replaced by a new phone.
    for (method, path, body) in signed_routes(&me) {
        assert_eq!(signed(&mut a, method, &path, &body).0, status::GONE, "deleted: {method} {path}");
    }
    let (mut b, _, b_name) = account(&hub);
    let _new = new_phone(Http::new(hub.url()), &b_name, &b);
    let b_user = b.user_id().to_owned();
    for (method, path, body) in signed_routes(&b_user) {
        assert_eq!(signed(&mut b, method, &path, &body).0, status::GONE, "replaced: {method} {path}");
    }
    assert_ne!(name, b_name);
}

/// Spec 2 (strict decoding), 6.3: a body that is not the route's type, empty, or with a byte after it is `400`.
#[test]
fn a_malformed_body_is_400_on_every_route() {
    let hub = TestHub::start();
    let (mut a, _, _) = account(&hub);
    let me = a.user_id().to_owned();
    for (method, path, body) in signed_routes(&me) {
        if body.is_empty() {
            continue;
        }
        let mut trailing = body.clone();
        trailing.push(0);
        for bad in [Vec::new(), vec![0xFF; 3], trailing] {
            let (code, _) = signed(&mut a, method, &path, &bad);
            assert_eq!(code, status::BAD_REQUEST, "{method} {path} {} bytes", bad.len());
        }
    }
    // Unsigned routes alike: registration, delivery (6.1: size and version only).
    for bad in [Vec::new(), vec![0xFF; 3]] {
        assert_eq!(raw(&hub, "POST", api::REGISTER, &[], bad).0, status::BAD_REQUEST);
    }
    let hash = yo_tests::devices_hash(&hub.url(), &me).unwrap();
    let deliver = |body: Vec<u8>| {
        raw(&hub, "POST", &api::deliver_path(&user_id(&me)), &[(api::HEADER_DEVICES, yo_wire::b64(&hash))], body).0
    };
    let mut unsealed = junk(1);
    unsealed[0] = 1;
    let mut longer = junk(1);
    longer.push(0);
    for bad in [Vec::new(), vec![yo_wire::envelope::SEALED; 1023], longer, unsealed] {
        assert_eq!(deliver(bad), status::BAD_REQUEST);
    }
    for size in yo_wire::envelope::BUCKETS {
        let mut envelope = vec![0; size];
        envelope[0] = yo_wire::envelope::SEALED;
        assert_eq!(deliver(envelope), status::ACCEPTED, "a bucket of {size}");
    }
    // The device check: missing or not 32 bytes of base64url.
    let path = api::deliver_path(&user_id(&me));
    assert_eq!(raw(&hub, "POST", &path, &[], junk(1)).0, status::BAD_REQUEST);
    for bad in ["!!", "AAAA", &yo_wire::b64(&[0; 33])] {
        assert_eq!(raw(&hub, "POST", &path, &[(api::HEADER_DEVICES, bad.to_owned())], junk(1)).0, status::BAD_REQUEST);
    }
    // A user id that is not 16 bytes of base64url is no route.
    for user in ["!!", "AAAA", &yo_wire::b64(&[0; 17])] {
        let (code, _) =
            raw(&hub, "POST", &format!("/v1/inbox/{user}"), &[(api::HEADER_DEVICES, yo_wire::b64(&hash))], junk(1));
        assert_eq!(code, status::NOT_FOUND, "{user}");
    }
}

/// Spec 6.6: a request body beyond 65 KiB is `413` on every route, and each route's own limits hold.
#[test]
fn bodies_are_held_to_their_limits() {
    let hub = TestHub::start();
    let (mut a, _, _) = account(&hub);
    let me = a.user_id().to_owned();
    let most = yo_wire::limits::MAX_BODY;
    for (method, path, _) in signed_routes(&me) {
        if method == "GET" || method == "DELETE" {
            continue;
        }
        // Refused before the body is read (spec 6.6): `413`, or the connection closed under a client still sending it.
        match a.signed_for_tests(method, &path, &vec![0; most + 1]) {
            Ok(response) => assert_eq!(response.status, status::TOO_LARGE, "{method} {path}"),
            Err(error) => assert!(matches!(error, Error::Network(_)), "{method} {path}: {error:?}"),
        }
    }
    let too_large = |path: &str, headers: Vec<(&'static str, String)>| {
        let mut request = reqwest::blocking::Client::new().post(format!("{}{path}", hub.url())).body(vec![2; most + 1]);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        request.send().map(|r| r.status().as_u16()).unwrap_or(status::TOO_LARGE)
    };
    assert_eq!(too_large(api::REGISTER, Vec::new()), status::TOO_LARGE);
    let hash = yo_tests::devices_hash(&hub.url(), &me).unwrap();
    let path = api::deliver_path(&user_id(&me));
    assert_eq!(too_large(&path, vec![(api::HEADER_DEVICES, yo_wire::b64(&hash))]), status::TOO_LARGE);

    // Prekeys (6.6): 32 one-time of each kind per upload, 2 KiB each, signatures of 64 bytes.
    let mut put = |prekeys: &api::Prekeys| signed(&mut a, "PUT", api::PREKEYS, &encode(prekeys)).0;
    let mut many = one_time(1_000, 32);
    assert_eq!(put(&many), status::NO_CONTENT);
    many.one_time.push(api::Prekey { id: 2_000, key: vec![1; 33].into() });
    assert_eq!(put(&many), status::TOO_LARGE, "33 one-time EC prekeys");
    let mut many = one_time(3_000, 32);
    many.kyber.push(signed_prekey(4_000, 1569));
    assert_eq!(put(&many), status::TOO_LARGE, "33 one-time Kyber prekeys");
    let none = one_time(0, 0);
    let bad = [
        api::Prekeys { signed: Some(signed_prekey(5_000, 2049)), ..none.clone() },
        api::Prekeys { last_resort: Some(signed_prekey(5_000, 0)), ..none.clone() },
        api::Prekeys {
            signed: Some(api::SignedPrekey { id: 5_000, key: vec![1; 33].into(), signature: vec![1; 65].into() }),
            ..none.clone()
        },
        api::Prekeys {
            last_resort: Some(api::SignedPrekey { id: 5_000, key: vec![1; 33].into(), signature: Vec::new().into() }),
            ..none.clone()
        },
        api::Prekeys { one_time: vec![api::Prekey { id: 5_000, key: vec![1; 2049].into() }], ..none.clone() },
        api::Prekeys { kyber: vec![signed_prekey(5_000, 0)], ..none.clone() },
    ];
    for prekeys in &bad {
        assert_eq!(put(prekeys), status::BAD_REQUEST, "{prekeys:?}");
    }
    assert_eq!(put(&api::Prekeys { signed: Some(signed_prekey(5_000, 2048)), ..none.clone() }), status::NO_CONTENT);
    assert_eq!(put(&none), status::NO_CONTENT, "an upload of nothing changes nothing");

    // Push token: 512 bytes.
    let push = |n: usize| encode(&api::Push { platform: api::PUSH_APNS, token: vec![b'a'; n].into() });
    assert_eq!(signed(&mut a, "PUT", api::PUSH, &push(512)).0, status::NO_CONTENT);
    assert_eq!(signed(&mut a, "PUT", api::PUSH, &push(513)).0, status::BAD_REQUEST);
    // Ack: 1000 ids.
    let ack = |n: u64| encode(&api::Ack { ids: (1..=n).collect() });
    assert_eq!(signed(&mut a, "POST", api::INBOX_ACK, &ack(1000)).0, status::NO_CONTENT);
    assert_eq!(signed(&mut a, "POST", api::INBOX_ACK, &ack(1001)).0, status::TOO_LARGE);
    // Storage: 60 KiB, not empty.
    let storage = |n: usize| encode(&api::Storage { version: 0, data: vec![1; n].into() });
    assert_eq!(signed(&mut a, "PUT", api::STORAGE, &storage(60 * 1024 + 1)).0, status::TOO_LARGE);
    assert_eq!(signed(&mut a, "PUT", api::STORAGE, &storage(0)).0, status::BAD_REQUEST);
    assert_eq!(signed(&mut a, "PUT", api::STORAGE, &storage(60 * 1024)).0, status::NO_CONTENT);
    // Lookup: at most 16 devices excepted; a handle that is none.
    let except = |n: usize| lookup_body(Handle::user(&user_id(&me)), 1, vec![[3; 32]; n]);
    assert_eq!(signed(&mut a, "POST", api::LOOKUP, &except(16)).0, status::OK);
    assert_eq!(signed(&mut a, "POST", api::LOOKUP, &except(17)).0, status::BAD_REQUEST);
    for handle in [
        Handle { kind: 9, value: vec![1; 16].into() },
        Handle { kind: yo_wire::handle::KIND_USER, value: vec![1; 15].into() },
        Handle { kind: yo_wire::handle::KIND_ACCOUNT_KEY, value: vec![1; 31].into() },
        Handle::username("_x"),
    ] {
        assert_eq!(signed(&mut a, "POST", api::LOOKUP, &lookup_body(handle, 0, Vec::new())).0, status::BAD_REQUEST);
    }
    // Report: a reason there is, not oneself.
    let report = |user: [u8; 16], reason| encode(&api::Report { user_id: user, reason });
    for reason in [0, 4, 255] {
        assert_eq!(signed(&mut a, "POST", api::REPORT, &report([1; 16], reason)).0, status::BAD_REQUEST);
    }
    assert_eq!(signed(&mut a, "POST", api::REPORT, &report(user_id(&me), 1)).0, status::BAD_REQUEST);
    // Feedback: text 1–2000 bytes of UTF-8, the version up to 64.
    let feedback = |text: Vec<u8>, version: usize| {
        encode(&api::Feedback { text: text.into(), app_version: vec![b'1'; version].into() })
    };
    assert_eq!(signed(&mut a, "POST", api::FEEDBACK, &feedback(vec![b'x'; 2000], 64)).0, status::NO_CONTENT);
    for body in [feedback(vec![b'x'; 2001], 1), feedback(Vec::new(), 1), feedback(b"x".to_vec(), 65)] {
        assert_eq!(signed(&mut a, "POST", api::FEEDBACK, &body).0, status::BAD_REQUEST);
    }
    // A crash report: the same body, no account, no signature (6.4).
    assert_eq!(raw(&hub, "POST", api::CRASH, &[], feedback(b"[crash] x".to_vec(), 9)).0, status::NO_CONTENT);
    assert_eq!(raw(&hub, "POST", api::CRASH, &[], feedback(vec![b'x'; 2001], 1)).0, status::BAD_REQUEST);
    // Backup (7): wraps 256 bytes, sealed 60 KiB, a locator of 16 bytes or none, auth keys that are keys.
    let mut record = backup_record(Vec::new());
    record.sealed = vec![1; 60 * 1024].into();
    assert_eq!(signed(&mut a, "PUT", api::BACKUP, &encode(&record)).0, status::NO_CONTENT);
    for change in 0..6 {
        let mut record = backup_record(Vec::new());
        match change {
            0 => record.sealed = vec![1; 60 * 1024 + 1].into(),
            1 => record.by_code = vec![1; 257].into(),
            2 => record.locator = vec![1; 15].into(),
            3 => record.auth_code = vec![1; 31].into(),
            4 => record.version = 1,
            _ => record.by_code = Vec::new().into(),
        }
        assert_eq!(signed(&mut a, "PUT", api::BACKUP, &encode(&record)).0, status::BAD_REQUEST, "change {change}");
    }
}

/// Spec 6.3, 6.4: registration answers `Registered`; the same request again (a device registered already) and a name
/// someone has are `409`; the account key again is its user (its old device `410`); a registration without its
/// signed or last-resort prekey, with a proof that does not hold or a push token too long is `400`.
#[test]
fn registration_answers_as_the_spec_says() {
    let hub = TestHub::start();
    let (a, http, name) = account(&hub);
    let body = http.sent.lock().unwrap().iter().find(|(_, p, _)| p == api::REGISTER).unwrap().2.clone();
    let request: api::Register = decode(&body).unwrap();
    assert_eq!(raw(&hub, "POST", api::REGISTER, &[], body.clone()).0, status::CONFLICT, "the same device again");
    let mut changed = request.clone();
    changed.device.prekeys.signed = None;
    assert_eq!(raw(&hub, "POST", api::REGISTER, &[], encode(&changed)).0, status::BAD_REQUEST, "no signed prekey");
    let mut changed = request.clone();
    changed.device.prekeys.last_resort = None;
    assert_eq!(raw(&hub, "POST", api::REGISTER, &[], encode(&changed)).0, status::BAD_REQUEST, "no last resort");
    let mut changed = request.clone();
    changed.device.device_id = [9; 16];
    assert_eq!(raw(&hub, "POST", api::REGISTER, &[], encode(&changed)).0, status::BAD_REQUEST, "proof for another");
    let mut changed = request.clone();
    changed.username = b"Someone_else".to_vec().into();
    assert_eq!(raw(&hub, "POST", api::REGISTER, &[], encode(&changed)).0, status::BAD_REQUEST, "proof for a name");
    let mut changed = request.clone();
    changed.device.certificate = vec![0; 96].into();
    assert_eq!(raw(&hub, "POST", api::REGISTER, &[], encode(&changed)).0, status::BAD_REQUEST, "no certificate");
    let mut changed = request.clone();
    changed.device.push.token = vec![1; 513].into();
    assert_eq!(raw(&hub, "POST", api::REGISTER, &[], encode(&changed)).0, status::BAD_REQUEST, "push token");
    let mut changed = request;
    changed.device.prekeys.one_time.push(api::Prekey { id: 99_999, key: vec![1; 33].into() });
    assert_eq!(raw(&hub, "POST", api::REGISTER, &[], encode(&changed)).0, status::TOO_LARGE, "33 one-time");
    // A name someone has, with another key: 409, also in another spelling.
    let taken =
        Client::register_username(Http::new(hub.url()), &name.to_lowercase(), None, None, yo_core::Push::none());
    assert_eq!(taken.err(), Some(Error::Refused(status::CONFLICT)));
    // The account key again: the same user, the answer names it; the device before is replaced.
    let mut old = a;
    let phone = new_phone(Http::new(hub.url()), &name, &old);
    assert_eq!(phone.user_id(), old.user_id());
    assert_eq!(phone.username(), Some(name.as_str()));
    assert_eq!(old.sync(0), Err(Error::Replaced));
}

/// Spec 3.3: how short a name may be is the hub's switch (`YO_NAME_MIN`): "Mia" with the default of 3, "Or" not;
/// one letter only once it is lowered to 1.
#[test]
fn the_least_name_length_is_the_hubs_switch() {
    let (bad, free) = (status::BAD_REQUEST, status::NO_CONTENT);
    for (least, expected) in [(None, (bad, bad, free)), (Some(1), (free, free, free))] {
        let mut config = Config::for_tests();
        if let Some(least) = least {
            config.name_min = least;
        }
        let hub = TestHub::start_with(config, "127.0.0.1:0");
        let check = |name: &str| raw(&hub, "GET", &api::username_path(name), &[], Vec::new()).0;
        assert_eq!((check("O"), check("Or"), check("🦄Mi")), expected, "least {least:?}");
    }
    let mut config = Config::for_tests();
    config.name_min = 3;
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let new = Client::register_username(Http::new(hub.url()), "Xy", None, None, yo_core::Push::none());
    assert_eq!(new.err(), Some(Error::Refused(status::BAD_REQUEST)), "a new two-letter name");
}

/// Spec 3.3: a name from before a higher least stays its owner's: found (`409`), sent to, restored and respelled;
/// only new names, at registration and on a rename, are measured.
#[test]
fn a_short_name_from_before_stays() {
    let dir = std::env::temp_dir().join(format!("yo-short-{}", random_u64()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut config = Config::for_tests();
    config.db_path = dir.join("hub.sqlite").to_string_lossy().into_owned();
    config.name_min = 2;
    let hub = TestHub::start_with(config.clone(), "127.0.0.1:0");
    let ya = register(Http::new(hub.url()), "Ya", None);
    hub.stop();

    config.name_min = 3;
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let check = |name: &str| raw(&hub, "GET", &api::username_path(name), &[], Vec::new()).0;
    assert_eq!(check("YA"), status::CONFLICT, "taken, not too short");
    assert_eq!(check("Ab"), status::BAD_REQUEST);
    assert_eq!(check("Abc"), status::NO_CONTENT);

    let (mut bob, _, _) = account(&hub);
    let mut ya = new_phone(Http::new(hub.url()), "Ya", &ya);
    bob.send_yo("Ya").unwrap();
    let events = ya.sync(0).unwrap();
    assert!(events.iter().any(|e| matches!(e, Event::Message { user, .. } if *user == bob.user_id())), "{events:?}");

    let set = |a: &mut Client, name: &str| {
        let body = encode(&api::SetUsername { username: Some(name.as_bytes().to_vec().into()) });
        signed(a, "PUT", api::USERNAME, &body).0
    };
    assert_eq!(set(&mut ya, "YA"), status::NO_CONTENT, "its own name respelled");
    assert_eq!(set(&mut bob, "Bo"), status::BAD_REQUEST, "a new short name");
    let _ = std::fs::remove_dir_all(dir);
}

/// Spec 6.3: `204` free, `409` taken (in any spelling) or reserved, `400` no username; the rate per address.
#[test]
fn whether_a_username_is_free() {
    let mut config = Config::for_tests();
    config.reserved = yo_hub::config::parse_reserved("Admin\n");
    config.limits.name_check_per_ip_minute = 8;
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let (_, _, name) = account(&hub);
    let check = |name: &str| raw(&hub, "GET", &api::username_path(name), &[], Vec::new()).0;
    assert_eq!(check("Free_one"), status::NO_CONTENT);
    assert_eq!(check(&name), status::CONFLICT);
    assert_eq!(check(&name.replace('0', "O")), status::CONFLICT, "a lookalike");
    assert_eq!(check("ADM1N"), status::CONFLICT, "reserved");
    let reserved = Client::register_username(Http::new(hub.url()), "Adm1n", None, None, yo_core::Push::none());
    assert_eq!(reserved.err(), Some(Error::Refused(status::CONFLICT)));
    for bad in ["a", "_abc", "a b c", "seventeen_letters"] {
        assert_eq!(check(bad), status::BAD_REQUEST, "{bad}");
    }
    assert_eq!(raw(&hub, "GET", "/v1/username/%FF%FE%FD", &[], Vec::new()).0, status::BAD_REQUEST);
    assert_eq!(check("Free_one"), status::RATE_LIMITED, "the ninth this minute");
}

/// Spec 3.3, 6.3: `PUT /v1/username` claims, renames and gives up; a name someone has or reserved is `409`; the same
/// request twice is the same; the old name is free at once.
#[test]
fn a_username_is_set_and_given_up() {
    let mut config = Config::for_tests();
    config.reserved = yo_hub::config::parse_reserved("Admin\n");
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let (mut a, _, name) = account(&hub);
    let (_, _, other) = account(&hub);
    let set = |a: &mut Client, name: Option<&str>| {
        let body = encode(&api::SetUsername { username: name.map(|n| n.as_bytes().to_vec().into()) });
        signed(a, "PUT", api::USERNAME, &body).0
    };
    let free = |name: &str| raw(&hub, "GET", &api::username_path(name), &[], Vec::new()).0 == status::NO_CONTENT;
    assert_eq!(set(&mut a, Some(&other)), status::CONFLICT);
    assert_eq!(set(&mut a, Some("admin")), status::CONFLICT);
    assert_eq!(set(&mut a, Some("x")), status::BAD_REQUEST);
    assert_eq!(set(&mut a, Some(&name.to_lowercase())), status::NO_CONTENT, "its own in another spelling");
    let renamed = format!("Re{}", &name[1..]);
    for _ in 0..2 {
        assert_eq!(set(&mut a, Some(&renamed)), status::NO_CONTENT);
    }
    assert!(free(&name) && !free(&renamed));
    for _ in 0..2 {
        assert_eq!(set(&mut a, None), status::NO_CONTENT);
    }
    assert!(free(&renamed));
    let me = a.user_id().to_owned();
    let (code, body) = signed(&mut a, "POST", api::LOOKUP, &lookup_body(Handle::user(&user_id(&me)), 2, Vec::new()));
    assert_eq!(code, status::OK);
    assert_eq!(decode::<api::LookupNamed>(&body).unwrap().username, None);
    assert_eq!(
        signed(&mut a, "POST", api::LOOKUP, &lookup_body(Handle::username(&renamed), 0, Vec::new())).0,
        status::NOT_FOUND
    );
}

/// Spec 6.3: a lookup by user id, account key and username answers who it is (`LookupResult`, or `LookupNamed` with
/// flag 2); with flag 1 a bundle per device; `404` for nobody. Each kind has its rate per device and day.
#[test]
fn lookups_answer_who_and_are_limited() {
    let hub = hub_with(|l| {
        l.lookup_per_device_day = 3;
        l.identity_lookup_per_device_day = 5;
    });
    let (mut a, _, _) = account(&hub);
    let (b, _, b_name) = account(&hub);
    let b_user = user_id(b.user_id());
    let mut ask =
        |handle: Handle, flags: u8| signed(&mut a, "POST", api::LOOKUP, &lookup_body(handle, flags, Vec::new()));
    let (code, body) = ask(Handle::user(&b_user), 0);
    assert_eq!(code, status::OK);
    let found: api::LookupResult = decode(&body).unwrap();
    assert_eq!((found.user_id, found.devices.len(), found.bundles.len()), (b_user, 1, 0));
    assert_eq!(found.devices[0].id, 1, "the first device of a user is number 1");
    let (code, body) = ask(Handle::account_key(&found.account_key), 2);
    assert_eq!(code, status::OK);
    let named: api::LookupNamed = decode(&body).unwrap();
    assert_eq!(named.found, found);
    assert_eq!(named.username.map(|n| n.as_slice().to_vec()), Some(b_name.as_bytes().to_vec()));
    let (code, body) = ask(Handle::username(&b_name.to_lowercase()), 2);
    assert_eq!((code, decode::<api::LookupNamed>(&body).unwrap().found.user_id), (status::OK, b_user));
    assert_eq!(ask(Handle::user(&[0; 16]), 0).0, status::NOT_FOUND);
    assert_eq!(ask(Handle::account_key(&[1; 32]), 0).0, status::NOT_FOUND, "the identity rate's fifth");
    assert_eq!(ask(Handle::user(&b_user), 0).0, status::RATE_LIMITED);
    // With prekeys: its own rate.
    let (code, body) = ask(Handle::user(&b_user), 1);
    assert_eq!(code, status::OK);
    let found: api::LookupResult = decode(&body).unwrap();
    assert_eq!(found.bundles.len(), 1);
    assert_eq!(found.bundles[0].device_key, found.devices[0].key);
    assert_eq!(ask(Handle::user(&b_user), 3).0, status::OK);
    assert_eq!(ask(Handle::username("Nobody_here"), 1).0, status::NOT_FOUND);
    assert_eq!(ask(Handle::user(&b_user), 1).0, status::RATE_LIMITED);
}

/// Spec 4.1, 6.5: one-time prekeys go once each, the last resort when none is left, a refill brings new ones, a
/// repeated upload none back; at most 64 of each kind, none with an id not above the highest so far; an upload with a
/// signed prekey or last resort replaces it.
#[test]
fn prekeys_are_handed_out_as_the_spec_says() {
    let hub = TestHub::start();
    let (mut target, http, _) = account(&hub);
    let (mut looker, _, _) = account(&hub);
    let who = target.user_id().to_owned();
    let target_key = bundles(&mut looker, &who, Vec::new()).devices[0].key;
    // That lookup took one of each; a lookup without prekeys or with the device excepted takes none.
    assert_eq!(prekeys_left(&mut target), 31);
    let (code, _) = signed(&mut looker, "POST", api::LOOKUP, &lookup_body(Handle::user(&user_id(&who)), 0, vec![]));
    assert_eq!(code, status::OK);
    assert!(bundles(&mut looker, &who, vec![target_key]).bundles.is_empty());
    assert_eq!(prekeys_left(&mut target), 31);
    let mut seen = std::collections::BTreeSet::new();
    let mut last_resort = None;
    for i in 0..40 {
        let found = bundles(&mut looker, &who, Vec::new());
        let bundle = &found.bundles[0];
        assert_eq!(bundle.device_key, target_key);
        match &bundle.one_time {
            Some(one) => {
                assert!(i < 31, "no more than were uploaded");
                assert!(seen.insert(one.id), "a one-time EC prekey handed out twice: {}", one.id);
                assert!(seen.insert(bundle.kyber.id), "a one-time Kyber prekey handed out twice");
            }
            None => {
                assert!(i >= 31);
                // The last resort, the same every time.
                assert_eq!(*last_resort.get_or_insert(bundle.kyber.id), bundle.kyber.id);
                assert!(!seen.contains(&bundle.kyber.id));
            }
        }
    }
    assert_eq!(prekeys_left(&mut target), 0);
    // The device's next sync sees 0 left and refills.
    target.sync(0).unwrap();
    target.after_sync().unwrap();
    assert_eq!(prekeys_left(&mut target), yo_wire::limits::PREKEY_TARGET);
    let upload = http.sent.lock().unwrap().iter().rev().find(|(_, p, _)| p == api::PREKEYS).unwrap().2.clone();
    let first = bundles(&mut looker, &who, Vec::new());
    let one = first.bundles[0].one_time.as_ref().unwrap().id;
    assert!(seen.iter().all(|id| *id < one), "refilled ids are above every id before");
    assert!(seen.insert(one));
    // The refill sent again (its answer lost): nothing comes back of what was handed out.
    assert_eq!(signed(&mut target, "PUT", api::PREKEYS, &upload).0, status::NO_CONTENT);
    assert_eq!(prekeys_left(&mut target), yo_wire::limits::PREKEY_TARGET - 1);
    for _ in 1..yo_wire::limits::PREKEY_TARGET {
        let found = bundles(&mut looker, &who, Vec::new());
        assert!(seen.insert(found.bundles[0].one_time.as_ref().unwrap().id));
    }
    assert!(bundles(&mut looker, &who, Vec::new()).bundles[0].one_time.is_none());

    // Hub-side bounds, with prekeys made up for a device of its own (the hub checks no signature, 6.5).
    let (mut s, _, _) = account(&hub);
    let mine = s.user_id().to_owned();
    for from in [1_000_000, 2_000_000, 3_000_000] {
        assert_eq!(signed(&mut s, "PUT", api::PREKEYS, &encode(&one_time(from, 32))).0, status::NO_CONTENT);
    }
    assert_eq!(prekeys_left(&mut s), 64, "at most 64 of each kind");
    // Ids not above the highest so far are not taken.
    for _ in 0..64 {
        bundles(&mut looker, &mine, Vec::new());
    }
    assert_eq!(prekeys_left(&mut s), 0);
    assert_eq!(signed(&mut s, "PUT", api::PREKEYS, &encode(&one_time(500, 32))).0, status::NO_CONTENT);
    assert_eq!(signed(&mut s, "PUT", api::PREKEYS, &encode(&one_time(2_000_001, 32))).0, status::NO_CONTENT);
    assert_eq!(prekeys_left(&mut s), 0, "ids used before");
    // A signed prekey and a last resort replace the ones before, and are handed out as uploaded.
    let rotated = api::Prekeys {
        signed: Some(signed_prekey(4_000_000, 33)),
        last_resort: Some(signed_prekey(4_000_001, 1569)),
        ..one_time(4_000_002, 1)
    };
    assert_eq!(signed(&mut s, "PUT", api::PREKEYS, &encode(&rotated)).0, status::NO_CONTENT);
    let bundle = bundles(&mut looker, &mine, Vec::new()).bundles.remove(0);
    assert_eq!(bundle.signed, rotated.signed.clone().unwrap());
    assert_eq!(bundle.kyber, rotated.kyber[0]);
    assert_eq!(bundle.one_time.as_ref(), rotated.one_time.first());
    let bundle = bundles(&mut looker, &mine, Vec::new()).bundles.remove(0);
    assert_eq!(
        (bundle.signed, bundle.kyber, bundle.one_time),
        (rotated.signed.unwrap(), rotated.last_resort.unwrap(), None)
    );
}

/// Spec 4.4: a delivery whose `Yo-Devices` is not the hub's roster stores nothing and is `409` with the roster; with
/// it, `202` to every device. A new phone changes the roster; a deleted account is `404`.
#[test]
fn a_delivery_names_the_devices_or_gets_them() {
    let hub = TestHub::start();
    let (mut b, _, b_name) = account(&hub);
    let (mut looker, _, _) = account(&hub);
    let who = b.user_id().to_owned();
    let path = api::deliver_path(&user_id(&who));
    let deliver = |hash: &[u8]| raw(&hub, "POST", &path, &[(api::HEADER_DEVICES, yo_wire::b64(hash))], junk(3));
    let (code, body) = deliver(&[0; 32]);
    assert_eq!(code, status::CONFLICT);
    let devices: api::Devices = decode(&body).unwrap();
    let found = bundles(&mut looker, &who, Vec::new());
    assert_eq!((devices.account_key, devices.devices.clone()), (found.account_key, found.devices.clone()));
    let hash = yo_wire::devices::hash(&devices.account_key, &devices.keys());
    assert_eq!(deliver(&hash).0, status::ACCEPTED);
    let (code, body) = signed(&mut b, "GET", &api::inbox_path(0), &[]);
    assert_eq!((code, decode::<api::Inbox>(&body).unwrap().items.len()), (status::OK, 1), "only the good one stored");
    // A new phone: the old roster is refused, with the new one.
    let mut phone = new_phone(Http::new(hub.url()), &b_name, &b);
    let (code, body) = deliver(&hash);
    assert_eq!(code, status::CONFLICT);
    let now: api::Devices = decode(&body).unwrap();
    assert_eq!(now.account_key, devices.account_key);
    assert_eq!(now.devices.len(), 1);
    assert_ne!(now.devices[0].key, devices.devices[0].key);
    assert_eq!(deliver(&yo_wire::devices::hash(&now.account_key, &now.keys())).0, status::ACCEPTED);
    assert_eq!(phone.sync(0).unwrap(), vec![], "junk, dropped");
    phone.delete_account().unwrap();
    assert_eq!(deliver(&hash).0, status::NOT_FOUND);
    assert_eq!(
        raw(&hub, "POST", &api::deliver_path(&[0; 16]), &[(api::HEADER_DEVICES, yo_wire::b64(&hash))], junk(1)).0,
        404
    );
}

/// Spec 6.6: deliveries 200 per address and minute (here 3); a full inbox `507` for that recipient only.
#[test]
fn deliveries_are_limited_per_address_and_per_inbox() {
    let hub = hub_with(|l| {
        l.deliver_per_ip_minute = 4;
        l.max_pending_per_device = 2;
    });
    let (a, _, _) = account(&hub);
    let (b, _, _) = account(&hub);
    // `deliver_raw` asks for the roster first: two deliveries each.
    assert_eq!(yo_tests::deliver_raw(&hub.url(), a.user_id(), junk(1)), status::ACCEPTED);
    assert_eq!(yo_tests::deliver_raw(&hub.url(), a.user_id(), junk(2)), status::ACCEPTED);
    let hash = yo_tests::devices_hash(&hub.url(), b.user_id());
    assert_eq!(hash, None, "the fifth this minute");
    let hub = hub_with(|l| l.max_pending_per_device = 2);
    let (mut a, _, _) = account(&hub);
    let (b, _, _) = account(&hub);
    for i in 0..2 {
        assert_eq!(yo_tests::deliver_raw(&hub.url(), a.user_id(), junk(i)), status::ACCEPTED);
    }
    assert_eq!(yo_tests::deliver_raw(&hub.url(), a.user_id(), junk(3)), status::INSUFFICIENT_STORAGE);
    assert_eq!(yo_tests::deliver_raw(&hub.url(), b.user_id(), junk(3)), status::ACCEPTED, "another inbox has room");
    assert_eq!(a.sync(0).unwrap(), vec![]);
    a.after_sync().unwrap();
    let key = a.access_key_for_tests();
    assert_eq!(
        yo_tests::deliver_raw_with(&hub.url(), a.user_id(), key.as_deref(), junk(4)),
        status::ACCEPTED,
        "read: room again"
    );
}

/// Spec 6.3, 6.6: the inbox answers oldest first, 100 at most, with the prekeys left; an ack deletes what it names
/// (again: nothing more); a long poll waits for its seconds; four requests of one device at once, not five.
#[test]
fn the_inbox_answers_and_takes_acks() {
    let hub = TestHub::start();
    let (mut a, _, _) = account(&hub);
    let hash = yo_tests::devices_hash(&hub.url(), a.user_id()).unwrap();
    let path = api::deliver_path(&user_id(a.user_id()));
    for i in 0..101u8 {
        let (code, _) = raw(&hub, "POST", &path, &[(api::HEADER_DEVICES, yo_wire::b64(&hash))], junk(i));
        assert_eq!(code, status::ACCEPTED);
    }
    let fetch = |a: &mut Client| decode::<api::Inbox>(&signed(a, "GET", &api::inbox_path(0), &[]).1).unwrap();
    let inbox = fetch(&mut a);
    assert_eq!((inbox.items.len(), inbox.prekeys_left), (100, 32));
    assert!(inbox.items.windows(2).all(|w| w[0].id < w[1].id), "oldest first");
    assert!(inbox.items.iter().enumerate().all(|(i, item)| item.envelope.as_slice() == junk(i as u8)));
    let ids: Vec<u64> = inbox.items.iter().map(|i| i.id).collect();
    for _ in 0..2 {
        assert_eq!(signed(&mut a, "POST", api::INBOX_ACK, &encode(&api::Ack { ids: ids.clone() })).0, 204);
    }
    let inbox = fetch(&mut a);
    assert_eq!(inbox.items.len(), 1);
    assert_eq!(inbox.items[0].envelope.as_slice(), junk(100));
    // An ack of ids not this device's deletes nothing of it.
    let (mut other, _, _) = account(&hub);
    assert_eq!(signed(&mut other, "POST", api::INBOX_ACK, &encode(&api::Ack { ids: vec![inbox.items[0].id] })).0, 204);
    assert_eq!(fetch(&mut a).items.len(), 1);
    signed(&mut a, "POST", api::INBOX_ACK, &encode(&api::Ack { ids: vec![inbox.items[0].id] }));
    // A long poll with nothing waits its seconds, and answers empty.
    let started = std::time::Instant::now();
    let (code, body) = signed(&mut a, "GET", &api::inbox_path(1), &[]);
    assert_eq!((code, decode::<api::Inbox>(&body).unwrap().items.len()), (status::OK, 0));
    assert!(started.elapsed() >= std::time::Duration::from_millis(900));
    let (code, _) = signed(&mut a, "GET", "/v1/inbox?wait=nonsense", &[]);
    assert_eq!(code, status::OK, "no number: no wait");
    // Four at once from one device, the fifth refused.
    let state = a.saved();
    let polls: Vec<_> = (0..4)
        .map(|_| {
            let mut copy = Client::restore(Http::new(hub.url()), &state).unwrap();
            std::thread::spawn(move || signed(&mut copy, "GET", &api::inbox_path(3), &[]).0)
        })
        .collect();
    // Each waits three seconds; one is enough to see them all under way.
    std::thread::sleep(std::time::Duration::from_secs(1));
    assert_eq!(hub.hub.under_way(), (1, 1));
    let mut fifth = Client::restore(Http::new(hub.url()), &state).unwrap();
    assert_eq!(signed(&mut fifth, "GET", &api::inbox_path(0), &[]).0, status::RATE_LIMITED);
    for poll in polls {
        assert_eq!(poll.join().unwrap(), status::OK);
    }
    assert_eq!(signed(&mut fifth, "GET", &api::inbox_path(0), &[]).0, status::OK, "places given back");
}

/// Spec 6.3, 6.9: the storage is `404` until written; a `PUT` names the version it replaces, else `409` with the
/// current one; the same write twice is the second's `409`; both directions count against 120 an hour.
#[test]
fn storage_is_versioned_and_limited() {
    let hub = hub_with(|l| l.storage_per_user_hour = 6);
    let (mut a, _, _) = account(&hub);
    assert_eq!(signed(&mut a, "GET", api::STORAGE, &[]).0, status::NOT_FOUND);
    let put = |version: u64, fill: u8| encode(&api::Storage { version, data: vec![fill; 1024].into() });
    assert_eq!(signed(&mut a, "PUT", api::STORAGE, &put(0, 1)).0, status::NO_CONTENT);
    let (code, body) = signed(&mut a, "PUT", api::STORAGE, &put(0, 1));
    assert_eq!(code, status::CONFLICT, "a repeat of a write that was taken");
    let current: api::Storage = decode(&body).unwrap();
    assert_eq!(current.data.as_slice(), vec![1; 1024]);
    assert_eq!(signed(&mut a, "PUT", api::STORAGE, &put(current.version, 2)).0, status::NO_CONTENT);
    let (code, body) = signed(&mut a, "GET", api::STORAGE, &[]);
    assert_eq!(code, status::OK);
    let stored: api::Storage = decode(&body).unwrap();
    assert!(stored.version > current.version);
    assert_eq!(stored.data.as_slice(), vec![2; 1024]);
    assert_eq!(signed(&mut a, "GET", api::STORAGE, &[]).0, status::OK);
    assert_eq!(signed(&mut a, "GET", api::STORAGE, &[]).0, status::RATE_LIMITED, "the seventh this hour");
    assert_eq!(signed(&mut a, "PUT", api::STORAGE, &put(stored.version, 3)).0, status::RATE_LIMITED);
}

/// Spec 6.3, 7: a backup is stored (`204`, again `204`), found only by a request signed with its door's key; a
/// locator another account has is `409`, a user id as locator too; an auth key that is no key `400`.
#[test]
fn backups_are_stored_once_per_locator() {
    let hub = TestHub::start();
    let (mut a, _, _) = account(&hub);
    let (mut b, _, _) = account(&hub);
    let record = encode(&backup_record(vec![4; 16]));
    for _ in 0..2 {
        assert_eq!(signed(&mut a, "PUT", api::BACKUP, &record).0, status::NO_CONTENT);
    }
    assert_eq!(signed(&mut b, "PUT", api::BACKUP, &record).0, status::CONFLICT, "another's locator");
    let theirs = encode(&backup_record(user_id(a.user_id()).to_vec()));
    assert_eq!(signed(&mut b, "PUT", api::BACKUP, &theirs).0, status::CONFLICT, "a user id as locator");
    let mut bad = backup_record(Vec::new());
    // Not a canonical Curve25519 key (its top bit set).
    bad.auth_code = {
        let mut key = yo_tests::account_key(&[9; 32]);
        key[31] |= 0x80;
        key.to_vec()
    }
    .into();
    assert_eq!(signed(&mut b, "PUT", api::BACKUP, &encode(&bad)).0, status::BAD_REQUEST);
    // A fetch unsigned, signed by the wrong key, or of nothing: 404 alike.
    assert_eq!(raw(&hub, "GET", &api::backup_path(&[4; 16]), &[], Vec::new()).0, status::NOT_FOUND);
    assert_eq!(raw(&hub, "GET", "/v1/backup/!!", &[], Vec::new()).0, status::NOT_FOUND);
    let time = yo_hub::now_ms();
    let path = api::backup_path(&[4; 16]);
    let sign = |seed: [u8; 32], time: u64| {
        let key = libsignal_protocol::PrivateKey::deserialize(&seed).unwrap();
        key.calculate_signature(&yo_wire::request::to_be_signed("GET", &path, time, &[]), &mut rand::rng()).unwrap()
    };
    // XEdDSA signs anew each time: a replay sends the same signature again.
    let fetch = |signature: &[u8], time: u64| {
        let headers = [("yo-time", time.to_string()), ("yo-signature", yo_wire::b64(signature))];
        raw(&hub, "GET", &path, &headers, Vec::new())
    };
    assert_eq!(fetch(&sign([8; 32], time), time).0, status::NOT_FOUND);
    let signature = sign([9; 32], time);
    let (code, body) = fetch(&signature, time);
    assert_eq!((code, body), (status::OK, record));
    assert_eq!(fetch(&signature, time).0, status::NOT_FOUND, "a replay");
    let old = time - 6 * 60 * 1000;
    assert_eq!(fetch(&sign([9; 32], old), old).0, status::NOT_FOUND, "out of the window");
}

/// Spec 6.3: a report `204` (again: the latest kept), `404` for no such user; feedback `204`; deleting the account
/// `204`, then `410`.
#[test]
fn reports_feedback_and_deletion() {
    let hub = TestHub::start();
    let (mut a, _, name) = account(&hub);
    let (mut b, _, _) = account(&hub);
    let report = |user: [u8; 16], reason| encode(&api::Report { user_id: user, reason });
    for reason in [api::REPORT_SPAM, api::REPORT_ABUSE, api::REPORT_OTHER] {
        assert_eq!(signed(&mut a, "POST", api::REPORT, &report(user_id(b.user_id()), reason)).0, status::NO_CONTENT);
    }
    assert_eq!(signed(&mut a, "POST", api::REPORT, &report([0; 16], 1)).0, status::NOT_FOUND);
    let feedback = encode(&api::Feedback { text: "Yo".as_bytes().to_vec().into(), app_version: Vec::new().into() });
    assert_eq!(signed(&mut a, "POST", api::FEEDBACK, &feedback).0, status::NO_CONTENT);
    assert_eq!(signed(&mut a, "DELETE", api::ACCOUNT, &[]).0, status::NO_CONTENT);
    assert_eq!(signed(&mut a, "DELETE", api::ACCOUNT, &[]).0, status::GONE);
    // Gone with it (3.2, 6.4): the user, its name, its key, which registers a new user now.
    let ask = |b: &mut Client, handle| signed(b, "POST", api::LOOKUP, &lookup_body(handle, 0, Vec::new())).0;
    assert_eq!(ask(&mut b, Handle::user(&user_id(a.user_id()))), status::NOT_FOUND);
    assert_eq!(ask(&mut b, Handle::username(&name)), status::NOT_FOUND);
    let again =
        Client::register_username(Http::new(hub.url()), &name, Some(a.account_secret()), None, yo_core::Push::none())
            .unwrap();
    assert_ne!(again.user_id(), a.user_id());
}

/// Spec 6.6: an old client's delivery token (`Authorization: PrivateToken`) is ignored, no `401`: the delivery counts
/// against its address like any other; there is no issuance any more.
#[test]
fn an_old_delivery_token_is_ignored() {
    // The roster's fetch is a delivery too: two this minute.
    let hub = hub_with(|l| l.deliver_per_ip_minute = 2);
    let (a, _, _) = account(&hub);
    let hash = yo_tests::devices_hash(&hub.url(), a.user_id()).unwrap();
    let path = api::deliver_path(&user_id(a.user_id()));
    let headers =
        [(api::HEADER_DEVICES, yo_wire::b64(&hash)), ("authorization", "PrivateToken token=\"AAAA\"".to_owned())];
    assert_eq!(raw(&hub, "POST", &path, &headers, junk(1)).0, status::ACCEPTED);
    assert_eq!(raw(&hub, "POST", &path, &headers, junk(2)).0, status::RATE_LIMITED, "by address all the same");
    assert_eq!(raw(&hub, "GET", "/.well-known/private-token-issuer-directory", &[], Vec::new()).0, status::NOT_FOUND);
    assert_eq!(raw(&hub, "POST", "/v1/tokens", &[], Vec::new()).0, status::NOT_FOUND);
}

/// Spec 6.6: 1200 signed requests per device and minute (here 5), counted only once the signature holds.
#[test]
fn signed_requests_are_limited_per_device() {
    let hub = hub_with(|l| l.signed_per_device_minute = 5);
    let (mut a, _, _) = account(&hub);
    let device = yo_wire::b64(&a.device_id());
    for _ in 0..10 {
        let headers = [
            ("yo-device", device.clone()),
            ("yo-time", yo_hub::now_ms().to_string()),
            ("yo-signature", yo_wire::b64(&[1; 64])),
        ];
        assert_eq!(raw(&hub, "GET", &api::inbox_path(0), &headers, Vec::new()).0, status::UNAUTHORIZED);
    }
    // The registration's sender certificate (spec 6.1) took one of the 5.
    for _ in 0..4 {
        assert_eq!(signed(&mut a, "PUT", api::PUSH, &encode(&api::Push::none())).0, status::NO_CONTENT);
    }
    assert_eq!(signed(&mut a, "PUT", api::PUSH, &encode(&api::Push::none())).0, status::RATE_LIMITED);
}

/// Spec 4.2 step 4, 6.4 (schema 1 → 2): a device without prekeys at the hub (an app from before sessions) has no
/// bundle; a Yo to it waits in the outbox and `send` is `Outdated`; the hub is asked again only after an hour, and
/// once the account has registered again with prekeys, the Yo goes.
#[test]
fn a_yo_to_an_account_without_prekeys_is_outdated_and_waits() {
    let dir = std::env::temp_dir().join(format!("yo-outdated-{}", random_u64()));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("hub.sqlite");
    let mut config = Config::for_tests();
    config.db_path = db_path.to_string_lossy().into_owned();
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let (b, _, b_name) = account(&hub);
    let (mut a, http, _) = account(&hub);
    let db = rusqlite::Connection::open(&db_path).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
    let removed = db
        .execute(
            "DELETE FROM prekeys WHERE device_id IN (SELECT id FROM devices WHERE user_id = ?1)",
            [user_id(b.user_id()).to_vec()],
        )
        .unwrap();
    assert!(removed > 2);
    drop(db);
    let found = bundles(&mut a, b.user_id(), Vec::new());
    assert_eq!((found.devices.len(), found.bundles.len()), (1, 0), "a device, no bundle");
    assert_eq!(a.send_yo(&b_name), Err(Error::Outdated));
    assert_eq!(a.waiting(), 1);
    assert!(a.contacts().iter().any(|c| c.outdated));
    let lookups = || http.sent.lock().unwrap().iter().filter(|(_, p, _)| p == api::LOOKUP).count();
    let before = lookups();
    a.maintenance().unwrap();
    assert_eq!(lookups(), before, "not asked again within the hour");
    assert_eq!(a.waiting(), 2, "the Yo, and now the profile key behind it");
    // The app updated: the account registers again with its key, a new device with prekeys.
    let mut phone = new_phone(Http::new(hub.url()), &b_name, &b);
    let later = yo_hub::now_ms() + 3600 * 1000 + 1;
    a.set_clock(Arc::new(move || later));
    a.maintenance().unwrap();
    assert_eq!(a.waiting(), 0);
    let me = a.user_id().to_owned();
    let events = phone.sync(0).unwrap();
    assert!(events.iter().any(|e| matches!(e, Event::Message { user, .. } if *user == me)), "{events:?}");
    drop(hub);
    let _ = std::fs::remove_dir_all(dir);
}

/// Spec 6.5: lookups at once from many devices never hand out one one-time prekey twice.
#[test]
fn one_time_prekeys_go_once_also_to_lookups_at_once() {
    let hub = TestHub::start();
    let (target, _, _) = account(&hub);
    let who = target.user_id().to_owned();
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let (mut looker, _, _) = account(&hub);
            let who = who.clone();
            std::thread::spawn(move || {
                (0..5)
                    .filter_map(|_| bundles(&mut looker, &who, Vec::new()).bundles[0].one_time.as_ref().map(|p| p.id))
                    .collect::<Vec<u32>>()
            })
        })
        .collect();
    let ids: Vec<u32> = threads.into_iter().flat_map(|t| t.join().unwrap()).collect();
    let distinct: std::collections::BTreeSet<u32> = ids.iter().copied().collect();
    assert_eq!(ids.len(), yo_wire::limits::PREKEY_TARGET as usize, "32 handed out, 8 lookups got the last resort");
    assert_eq!(distinct.len(), ids.len(), "none twice");
}

/// Spec 6.3: a replaced device is answered `410` for a year, then `401`, as a device the hub never knew.
#[test]
fn a_replaced_device_is_gone_for_a_year() {
    const DAY: u64 = 24 * 3600 * 1000;
    let hub = TestHub::start();
    let (mut old, _, name) = account(&hub);
    let mut phone = new_phone(Http::new(hub.url()), &name, &old);
    phone.put_backup(&backup_record(Vec::new())).unwrap();
    let now = yo_hub::now_ms();
    hub.hub.prune(now + 364 * DAY).unwrap();
    assert_eq!(signed(&mut old, "GET", &api::inbox_path(0), &[]).0, status::GONE);
    hub.hub.prune(now + 366 * DAY).unwrap();
    assert_eq!(signed(&mut old, "GET", &api::inbox_path(0), &[]).0, status::UNAUTHORIZED);
    assert_eq!(signed(&mut phone, "GET", &api::inbox_path(0), &[]).0, status::OK, "secured: kept");
}

/// A body that says it is larger than any request may be is refused before it is read (spec 6.6): `413` at once,
/// with not a byte of it sent, on a route with a small body and on a delivery.
#[test]
fn a_body_too_large_is_refused_before_it_is_read() {
    use std::io::{Read, Write};
    let hub = TestHub::start();
    let host = hub.url().trim_start_matches("http://").trim_end_matches('/').to_owned();
    for path in [api::REGISTER.to_owned(), api::deliver_path(&[7; 16])] {
        let mut stream = std::net::TcpStream::connect(&host).unwrap();
        stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        let head = format!("POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: 10000000\r\n\r\n");
        stream.write_all(head.as_bytes()).unwrap();
        let mut answer = [0u8; 64];
        let n = stream.read(&mut answer).expect("answered without the body");
        let line = String::from_utf8_lossy(&answer[..n]);
        assert!(line.starts_with("HTTP/1.1 413"), "{path}: {line}");
    }
}

#[test]
fn delivery_urgency_selects_alert_or_background_and_defaults_to_alert() {
    let config = Config {
        apns: Some(yo_hub::config::ApnsConfig {
            key_pem: include_str!("../fixtures/test-apns.p8").into(),
            key_id: "KEYID12345".into(),
            team_id: "TEAMID1234".into(),
            topic: "co.justyo.app".into(),
        }),
        ..Config::for_tests()
    };
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let name = username(random_u64() % 1_000_000_000);
    let client = Client::register_username(
        Http::new(hub.url()),
        &name,
        None,
        None,
        yo_core::Push { platform: api::PUSH_APNS, token: b"a1b2c3".to_vec().into() },
    )
    .unwrap();
    let user = client.user_id();
    let hash = yo_tests::devices_hash(&hub.url(), user).unwrap();
    let path = api::deliver_path(&user_id(user));
    for (index, value) in [None, Some("true"), Some("false")].into_iter().enumerate() {
        let mut headers = vec![(api::HEADER_DEVICES, yo_wire::b64(&hash))];
        if let Some(value) = value {
            headers.push((api::HEADER_URGENT, value.into()));
        }
        assert_eq!(raw(&hub, "POST", &path, &headers, junk(index as u8)).0, status::ACCEPTED);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if hub.push.sent.lock().unwrap().len() > index {
                break;
            }
            assert!(std::time::Instant::now() < until, "push sent");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let sent = hub.push.sent.lock().unwrap();
        let push = &sent[index];
        let header = |name| push.headers.iter().find(|(n, _)| n == name).unwrap().1.as_str();
        let body: serde_json::Value = serde_json::from_slice(&push.body).unwrap();
        if value == Some("false") {
            assert_eq!(header("apns-push-type"), "background");
            assert_eq!(header("apns-priority"), "5");
            assert_eq!(body, serde_json::json!({"aps": {"content-available": 1}}));
        } else {
            assert_eq!(header("apns-push-type"), "alert");
            assert_eq!(header("apns-priority"), "10");
            assert_eq!(body["aps"]["alert"]["body"], "You may have received a Yo");
            assert_eq!(body["aps"]["sound"], "yo.caf");
            assert_eq!(body["aps"]["mutable-content"], 1);
        }
    }
    assert_eq!(
        raw(
            &hub,
            "POST",
            &path,
            &[(api::HEADER_DEVICES, yo_wire::b64(&hash)), (api::HEADER_URGENT, "invalid".into())],
            junk(4)
        )
        .0,
        status::BAD_REQUEST
    );
}

#[test]
fn support_threads_are_private_and_read_markers_are_bounded() {
    let config = Config { admin_token: Some("test-token".into()), ..Config::for_tests() };
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let (mut a, _, _) = account(&hub);
    let (mut b, _, _) = account(&hub);
    a.feedback_begin("hello 🎨", "1").and_then(|r| r.send()).unwrap();
    assert!(b.support_begin(None).and_then(|r| r.thread()).unwrap().messages.is_empty());
    let headers = [("yo-admin-token", "test-token".into())];
    assert_eq!(raw(&hub, "GET", "/v1/admin/messages", &[], vec![]).0, 404);
    let (_, body) = raw(&hub, "GET", "/v1/admin/messages", &headers, vec![]);
    let threads: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // Only users' threads: the team's to-dos are a table of their own (/v1/admin/todos).
    assert_eq!(threads.as_array().unwrap().len(), 1);
    assert_eq!(threads[0]["unread"], 1);
    let path = format!("/v1/admin/messages/{}", threads[0]["user_id"].as_str().unwrap());
    assert_eq!(raw(&hub, "POST", &path, &headers, b"team reply".to_vec()).0, 204);
    let first = a.support_begin(None).and_then(|r| r.thread()).unwrap();
    assert_eq!(first.unread, 1);
    assert_eq!(first.messages.len(), 2);
    assert_eq!(first.messages[1].text.as_slice(), b"team reply");
    assert_eq!(raw(&hub, "POST", &path, &headers, b"later reply".to_vec()).0, 204);
    a.support_begin(Some(first.messages[1].id)).and_then(|r| r.send()).unwrap();
    assert_eq!(
        a.support_begin(None).and_then(|r| r.thread()).unwrap().unread,
        1,
        "reply arriving after fetch stays unread"
    );
    b.support_begin(Some(u64::MAX >> 1)).and_then(|r| r.send()).unwrap();
    assert_eq!(
        a.support_begin(None).and_then(|r| r.thread()).unwrap().unread,
        1,
        "another account cannot mark this thread read"
    );
    let handled = format!("{path}/handled");
    assert_eq!(raw(&hub, "POST", &handled, &headers, first.messages[0].id.to_string().into_bytes()).0, 204);
    let (_, body) = raw(&hub, "GET", "/v1/admin/messages", &headers, vec![]);
    let threads: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(threads[0]["unread"], 0);
    for message in &first.messages {
        let done = format!("/v1/admin/messages/done/{}", message.id);
        assert_eq!(raw(&hub, "POST", &done, &[], b"1".to_vec()).0, 404);
        assert_eq!(raw(&hub, "POST", &done, &headers, b"bad".to_vec()).0, 400);
        for value in [b"1", b"0"] {
            assert_eq!(raw(&hub, "POST", &done, &headers, value.to_vec()).0, 204);
            let (_, body) = raw(&hub, "GET", &path, &headers, vec![]);
            let thread: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let row = thread["messages"].as_array().unwrap().iter().find(|m| m["id"] == message.id).unwrap();
            assert_eq!(row["done_at"].is_null(), value == b"0");
        }
    }
    assert_eq!(a.support_begin(None).and_then(|r| r.thread()).unwrap().unread, 1);
    assert_eq!(raw(&hub, "POST", "/v1/admin/todos", &headers, b"team task".to_vec()).0, 201);
    let (_, body) = raw(&hub, "GET", "/v1/admin/todos", &headers, vec![]);
    let todos: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let done = format!("/v1/admin/todos/{}", todos[0]["id"]);
    assert_eq!(raw(&hub, "POST", &done, &headers, b"1".to_vec()).0, 204);
    assert_eq!(raw(&hub, "POST", &path, &headers, vec![b'x'; 2001]).0, 400);
    assert_eq!(raw(&hub, "GET", "/v1/admin/messages/invalid", &headers, vec![]).0, 400);
}

#[test]
fn support_and_legacy_feedback_share_the_user_rate_limit() {
    let hub = hub_with(|limits| limits.feedback_per_user_day = 2);
    let (mut a, _, _) = account(&hub);
    let (mut b, _, _) = account(&hub);
    a.send_support_begin("new route", "1").unwrap().send().unwrap();
    a.feedback_begin("legacy route", "1").and_then(|r| r.send()).unwrap();
    assert_eq!(a.send_support_begin("limited", "1").unwrap().send(), Err(Error::Refused(status::RATE_LIMITED)));
    b.send_support_begin("separate user", "1").unwrap().send().unwrap();
    assert_eq!(a.support_begin(None).and_then(|r| r.thread()).unwrap().messages.len(), 2);
    assert_eq!(
        signed(&mut a, "POST", api::SUPPORT_READ, &encode(&api::SupportRead { through: u64::MAX })).0,
        status::BAD_REQUEST
    );
}

#[test]
fn diagnostics_are_anonymous_count_all_and_admin_ticks_require_a_token() {
    let mut config = Config::for_tests();
    config.admin_token = Some("test-token".into());
    config.limits.crash_per_ip_day = 1;
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let error = serde_json::json!({"domain":"network", "code":"timeout", "place":"sync", "app":"ios/1", "os":"26", "model":"iPhone"}).to_string().into_bytes();
    for _ in 0..12 {
        assert_eq!(raw(&hub, "POST", "/v1/errors", &[], error.clone()).0, status::NO_CONTENT);
    }
    for body in [b"{}".to_vec(), b"null".to_vec(), b"{bad".to_vec(), serde_json::json!({"app":"ios/1","os":"26","model":"iPhone","domain":"test","code":"1","place":"sync","user":"forbidden"}).to_string().into_bytes()] {
        assert_eq!(raw(&hub, "POST", "/v1/errors", &[], body).0, status::BAD_REQUEST);
    }
    assert_eq!(raw(&hub, "POST", "/v1/errors", &[], vec![b'x'; 60 * 1024 + 1]).0, 413);
    // A day's unread notifications, counted on the phone: one report, the count it carries.
    let fallback = |count: &str, severity: Option<&str>| {
        let mut body = serde_json::json!({"domain":"NotificationFallback", "code":"lockedBeforeFirstUnlock",
            "place":"NotificationService", "app":"ios/1", "os":"26", "model":"iPhone", "count":count, "details":"2026-11-18"});
        if let Some(severity) = severity {
            body["severity"] = severity.into();
        }
        body.to_string().into_bytes()
    };
    for (count, severity) in [("0", Some("expected")), ("x", Some("expected")), ("100001", None), ("3", Some("loud"))] {
        assert_eq!(raw(&hub, "POST", "/v1/errors", &[], fallback(count, severity)).0, status::BAD_REQUEST);
    }
    for _ in 0..2 {
        assert_eq!(raw(&hub, "POST", "/v1/errors", &[], fallback("5", Some("expected"))).0, status::NO_CONTENT);
    }
    let path = "/v1/admin/diagnostics";
    assert_eq!(raw(&hub, "GET", path, &[], vec![]).0, 404);
    assert_eq!(raw(&hub, "PUT", path, &[("Yo-Admin-Token", "wrong".into())], b"{}".to_vec()).0, 404);
    let headers = [("Yo-Admin-Token", "test-token".into())];
    let (status, body) = raw(&hub, "GET", path, &headers, vec![]);
    assert_eq!(status, 200);
    let rows: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let rows = rows.as_array().unwrap();
    let with = |code: &str| rows.iter().find(|r| r["payload"].as_str().unwrap().contains(code)).unwrap().clone();
    assert_eq!(with("timeout")["count"], 12);
    let locked = with("lockedBeforeFirstUnlock");
    assert_eq!(locked["count"], 10);
    assert!(locked["payload"].as_str().unwrap().contains(r#""severity":"expected""#));
    let id = with("timeout")["id"].clone();
    let (status, body) =
        raw(&hub, "PUT", path, &headers, serde_json::json!({"id":id,"done":true}).to_string().into_bytes());
    assert_eq!(status, 200);
    let rows: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(rows.as_array().unwrap().iter().find(|r| r["id"] == id).unwrap()["done"], true);
}

#[test]
fn the_live_stream_is_the_operators_and_tells_each_yo_at_once() {
    use std::io::BufRead;
    let config = Config { admin_token: Some("test-token".into()), ..Config::for_tests() };
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    assert_eq!(raw(&hub, "GET", "/v1/admin/live", &[], vec![]).0, 404);
    assert_eq!(raw(&hub, "GET", "/v1/admin/live", &[("yo-admin-token", "wrong".into())], vec![]).0, 404);
    let client = reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(20)).build().unwrap();
    let stream =
        client.get(format!("{}/v1/admin/live", hub.url())).header("yo-admin-token", "test-token").send().unwrap();
    assert_eq!(stream.status().as_u16(), 200);
    assert!(stream.headers()["content-type"].to_str().unwrap().starts_with("text/event-stream"));
    let (a, _, _) = account(&hub);
    let hash = yo_tests::devices_hash(&hub.url(), a.user_id()).unwrap();
    let path = api::deliver_path(&user_id(a.user_id()));
    for i in 0..3u8 {
        let (code, _) = raw(&hub, "POST", &path, &[(api::HEADER_DEVICES, yo_wire::b64(&hash))], junk(i));
        assert_eq!(code, status::ACCEPTED);
    }
    // Each Yo as it is counted, long before the minute's flush; its kind only, nothing of who.
    let events: Vec<String> = std::io::BufReader::new(stream)
        .lines()
        .map(Result::unwrap)
        .filter(|line| line.starts_with("data:"))
        .take(3)
        .collect();
    assert_eq!(events, [r#"data: {"k":"yo"}"#; 3]);
}
