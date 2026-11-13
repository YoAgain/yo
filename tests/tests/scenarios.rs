//! Scenarios: a real hub, real clients over HTTP, the protocol end to end (spec/v1.md).

use std::sync::Arc;
use yo_core::{Client, Content, Error, Event, Push};
use yo_tests::{deliver_raw, phone, random_u64, register, user_of, Http, TestHub};
use yo_wire::api;

fn pair(hub: &TestHub) -> (Client, Client, String, String) {
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (phone(n), phone(n + 1));
    let a = register(Http::new(hub.url()), &pa, Some("ALICE"));
    let b = register(Http::new(hub.url()), &pb, Some("BOB"));
    (a, b, pa, pb)
}

fn digits(p: &str) -> String {
    user_of(p)
}

fn yos_from(events: &[Event], from: &str) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::Message { user, content: Content::Yo, .. } if *user == digits(from)))
        .count()
}

#[test]
fn a_yo_goes_there_and_back() {
    let hub = TestHub::start();
    let (mut a, mut b, pa, pb) = pair(&hub);
    a.send_yo(&pb).unwrap();
    let events = b.sync(0).unwrap();
    assert!(events.contains(&Event::NewChat { user: digits(&pa) }), "{events:?}");
    assert!(events.contains(&Event::Profile { user: digits(&pa), name: "ALICE".into() }), "{events:?}");
    assert_eq!(yos_from(&events, &pa), 1);
    b.send_yo(&pa).unwrap();
    let events = a.sync(0).unwrap();
    assert_eq!(yos_from(&events, &pb), 1, "{events:?}");
    // B answered by A's number, which it had not known A by: the same chat, not a second one.
    assert_eq!((a.chats().len(), b.chats().len()), (1, 1));
    assert!(b.contacts()[0].handles.contains(&pa));
    // B's first send updated its leaf (it joined by Welcome): the chat moved to epoch 2 on both sides.
    assert_eq!(a.chats()[0].2, 2);
    assert_eq!(b.chats()[0].2, 2);
    assert_eq!(a.dropped() + b.dropped(), 0);
    assert_eq!(a.pending() + b.pending(), 0);
}

#[test]
fn long_poll_wakes_on_delivery() {
    let hub = TestHub::start();
    let (mut a, mut b, pa, pb) = pair(&hub);
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    // B's first send is preceded by its update commit: the poll wakes on whichever envelope comes first.
    let from = pb.clone();
    let waiter = std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let mut events = Vec::new();
        while yos_from(&events, &from) == 0 && started.elapsed() < std::time::Duration::from_secs(20) {
            events.extend(a.sync(20).unwrap());
        }
        (events, started.elapsed())
    });
    std::thread::sleep(std::time::Duration::from_millis(300));
    b.send_yo(&pa).unwrap();
    let (events, waited) = waiter.join().unwrap();
    assert_eq!(yos_from(&events, &pb), 1);
    assert!(waited < std::time::Duration::from_secs(5), "{waited:?}");
}

#[test]
fn numbers_not_on_yo_and_sending_to_oneself() {
    let hub = TestHub::start();
    let (mut a, _, pa, _) = pair(&hub);
    assert_eq!(a.send_yo(&phone(999_999_999)), Err(Error::NotOnYo));
    assert!(a.send_yo(&pa).is_err());
    assert_eq!(a.send_yo("+0170 123"), Err(Error::Phone));
    // Without "+" it is a username, and this is no valid one.
    assert!(a.send_yo("0170 123").is_err());
}

#[test]
fn a_hundred_messages_heal_the_chat() {
    let hub = TestHub::start();
    let (mut a, mut b, pa, pb) = pair(&hub);
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    b.send_yo(&pa).unwrap();
    a.sync(0).unwrap();
    let before = a.chats()[0].2;
    for _ in 0..=yo_core::UPDATE_AFTER_MESSAGES {
        a.send_yo(&pb).unwrap();
    }
    let events = b.sync(0).unwrap();
    assert_eq!(yos_from(&events, &pa), yo_core::UPDATE_AFTER_MESSAGES as usize + 1);
    assert_eq!(a.chats()[0].2, before + 1, "one update after a hundred messages");
    assert_eq!(b.chats()[0].2, before + 1);
    assert_eq!(b.dropped(), 0);
}

#[test]
fn a_week_heals_the_chat() {
    let hub = TestHub::start();
    let (mut a, mut b, pa, pb) = pair(&hub);
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    b.send_yo(&pa).unwrap();
    a.sync(0).unwrap();
    let before = a.chats()[0].2;
    let later = yo_hub::now_ms() + yo_core::UPDATE_AFTER_MS + 1;
    a.set_clock(Arc::new(move || later));
    a.maintenance().unwrap();
    b.sync(0).unwrap();
    assert_eq!(a.chats()[0].2, before + 1);
    assert_eq!(b.chats()[0].2, before + 1);
    b.send_yo(&pa).unwrap();
    assert_eq!(yos_from(&a.sync(0).unwrap(), &pb), 1);
}

#[test]
fn simultaneous_updates_are_ordered_by_the_hub() {
    let hub = TestHub::start();
    let (mut a, mut b, pa, pb) = pair(&hub);
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    b.send_yo(&pa).unwrap();
    a.sync(0).unwrap();
    for round in 0..10 {
        let later = yo_hub::now_ms() + (round + 1) * (yo_core::UPDATE_AFTER_MS + 1);
        a.set_clock(Arc::new(move || later));
        b.set_clock(Arc::new(move || later));
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let (ba, bb) = (barrier.clone(), barrier);
        let ta = std::thread::spawn(move || {
            ba.wait();
            a.maintenance().unwrap();
            a
        });
        let tb = std::thread::spawn(move || {
            bb.wait();
            b.maintenance().unwrap();
            b
        });
        a = ta.join().unwrap();
        b = tb.join().unwrap();
        a.sync(0).unwrap();
        b.sync(0).unwrap();
        assert_eq!(a.chats()[0].2, b.chats()[0].2, "round {round}: both in the same epoch");
        // A send receives first: what arrived meanwhile comes back with it.
        let mut at_a = a.send_yo(&pb).unwrap();
        let mut at_b = b.send_yo(&pa).unwrap();
        at_b.extend(b.sync(0).unwrap());
        at_a.extend(a.sync(0).unwrap());
        assert_eq!(yos_from(&at_b, &pa), 1, "round {round}");
        assert_eq!(yos_from(&at_a, &pb), 1, "round {round}");
    }
    assert_eq!(a.dropped() + b.dropped(), 0, "{:?} {:?}", a.drop_reasons(), b.drop_reasons());
}

#[test]
fn two_chats_started_at_once_both_work() {
    let hub = TestHub::start();
    let (a, b, pa, pb) = pair(&hub);
    let (pa2, pb2) = (pa.clone(), pb.clone());
    let ta = std::thread::spawn(move || {
        let mut a = a;
        let events = a.send_yo(&pb2).unwrap();
        (a, events)
    });
    let tb = std::thread::spawn(move || {
        let mut b = b;
        let events = b.send_yo(&pa2).unwrap();
        (b, events)
    });
    let ((mut a, mut at_a), (mut b, mut at_b)) = (ta.join().unwrap(), tb.join().unwrap());
    at_a.extend(a.sync(0).unwrap());
    at_b.extend(b.sync(0).unwrap());
    assert_eq!(yos_from(&at_a, &pb), 1);
    assert_eq!(yos_from(&at_b, &pa), 1);
    let mut at_a = a.send_yo(&pb).unwrap();
    let mut at_b = b.send_yo(&pa).unwrap();
    at_b.extend(b.sync(0).unwrap());
    at_a.extend(a.sync(0).unwrap());
    assert_eq!(yos_from(&at_b, &pa), 1);
    assert_eq!(yos_from(&at_a, &pb), 1);
    assert_eq!(a.dropped() + b.dropped(), 0);
}

#[test]
fn a_new_phone_without_passkey_keeps_the_chat_with_a_new_key() {
    let hub = TestHub::start();
    let (mut a, mut b, pa, pb) = pair(&hub);
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    // B registers again: a new device, a new account key.
    let mut b2 = register(Http::new(hub.url()), &pb, Some("BOB"));
    assert_eq!(b2.user_id(), b.user_id(), "a new phone keeps the user id");
    assert_eq!(b.sync(0), Err(Error::Replaced));
    let chat = a.chats()[0].1;
    // A's Yo meets B's new device at the hub (409): A replaces B's old device by the new one in the same chat.
    let events = a.send_yo(&pb).unwrap();
    assert!(events.contains(&Event::IdentityChanged { user: digits(&pb) }), "{events:?}");
    let events = b2.sync(0).unwrap();
    assert!(events.contains(&Event::NewChat { user: digits(&pa) }));
    assert_eq!(yos_from(&events, &pa), 1, "the held Yo, encrypted anew for the new device");
    b2.send_yo(&pa).unwrap();
    assert_eq!(yos_from(&a.sync(0).unwrap(), &pb), 1);
    assert_eq!(a.chats().len(), 1, "the same chat goes on");
    assert_eq!(a.chats()[0].1, chat);
    assert_eq!(b2.chats()[0].1, chat);
    assert_eq!(a.chats()[0].3, 2, "two members: A and B's new device");
    assert_eq!(a.dropped() + b2.dropped(), 0, "{:?} {:?}", a.drop_reasons(), b2.drop_reasons());
}

#[test]
fn a_new_phone_with_the_same_account_key_is_seen_by_nobody() {
    let hub = TestHub::start();
    let (mut a, mut b, pa, pb) = pair(&hub);
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    a.set_verified(&pb, true).unwrap();
    // B's account key comes back on a new phone (what a passkey will do).
    let http = Http::new(hub.url());
    Client::request_code(http.as_ref(), &pb).unwrap();
    let mut b2 = Client::register_keeping_key(http, &pb, "000000", b.account_secret()).unwrap();
    let events = a.send_yo(&pb).unwrap();
    assert!(!events.iter().any(|e| matches!(e, Event::IdentityChanged { .. })), "{events:?}");
    assert!(a.contacts()[0].verified, "the verification holds: the key is the same");
    assert_eq!(yos_from(&b2.sync(0).unwrap(), &pa), 1);
    b2.send_yo(&pa).unwrap();
    assert_eq!(yos_from(&a.sync(0).unwrap(), &pb), 1);
}

#[test]
fn a_verified_contact_with_a_new_key_waits_for_confirmation() {
    let hub = TestHub::start();
    let (mut a, mut b, pa, pb) = pair(&hub);
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    a.set_verified(&pb, true).unwrap();
    let mut b2 = register(Http::new(hub.url()), &pb, Some("BOB"));
    // Nothing goes to the new key until A confirms it: the warning before the message.
    assert_eq!(a.send_yo(&pb), Err(Error::Unconfirmed));
    let contact = &a.contacts()[0];
    assert!(contact.unconfirmed && !contact.verified);
    assert_eq!(yos_from(&b2.sync(0).unwrap(), &pa), 0);
    a.confirm_identity(&pb).unwrap();
    a.send_yo(&pb).unwrap();
    assert_eq!(yos_from(&b2.sync(0).unwrap(), &pa), 2, "the held Yo and the new one");
}

#[test]
fn the_last_resort_keeps_a_number_reachable_and_is_refilled() {
    let hub = TestHub::start();
    let base = random_u64() % 1_000_000_000;
    let target = phone(base);
    let mut c = register(Http::new(hub.url()), &target, None);
    let senders = yo_wire::limits::KEY_PACKAGE_TARGET as u64 + 5;
    for i in 1..=senders {
        let mut s = register(Http::new(hub.url()), &phone(base + i), None);
        s.send_yo(&target).unwrap();
    }
    let events = c.sync(0).unwrap();
    assert_eq!(events.iter().filter(|e| matches!(e, Event::NewChat { .. })).count() as u64, senders);
    assert_eq!(c.dropped(), 0);
    // The sync refilled: one more round of senders gets single-use KeyPackages again.
    let mut s = register(Http::new(hub.url()), &phone(base + senders + 1), None);
    s.send_yo(&target).unwrap();
    assert_eq!(c.sync(0).unwrap().iter().filter(|e| matches!(e, Event::NewChat { .. })).count(), 1);
}

#[test]
fn a_signed_request_works_once() {
    let hub = TestHub::start();
    let http = Http::new(hub.url());
    let n = random_u64() % 1_000_000_000;
    let mut a = register(http.clone(), &phone(n), None);
    a.sync(0).unwrap();
    let (method, path, headers, body) = http.recorded.lock().unwrap().last().cloned().unwrap();
    let client = reqwest::blocking::Client::new();
    let mut replay = client.request(method.parse().unwrap(), format!("{}{}", hub.url(), path)).body(body);
    for (name, value) in headers {
        replay = replay.header(name, value);
    }
    assert_eq!(replay.send().unwrap().status().as_u16(), api::status::UNAUTHORIZED);
}

#[test]
fn a_forged_commit_is_refused_and_the_chat_lives_on() {
    let hub = TestHub::start();
    let (mut a, mut b, pa, pb) = pair(&hub);
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    let (_, group, epoch, _) = a.chats()[0].clone();
    let client = reqwest::blocking::Client::new();
    let post = |body: Vec<u8>| {
        client.post(format!("{}{}", hub.url(), api::commit_path(&group))).body(body).send().unwrap().status().as_u16()
    };
    let envelope = yo_wire::envelope::seal(b"forged").unwrap();
    let forged = api::Commit { epoch, token: [9; 32], next_token_hash: [9; 32], envelope: envelope.clone().into() };
    assert_eq!(post(yo_wire::encode(&forged)), api::status::FORBIDDEN);
    let stale = api::Commit { epoch: epoch - 1, token: [9; 32], next_token_hash: [9; 32], envelope: envelope.into() };
    assert_eq!(post(yo_wire::encode(&stale)), api::status::CONFLICT);
    b.send_yo(&pa).unwrap();
    assert_eq!(yos_from(&a.sync(0).unwrap(), &pb), 1);
}

#[test]
fn garbage_from_a_stranger_is_dropped_and_harms_nothing() {
    let hub = TestHub::start();
    let (mut a, mut b, pa, pb) = pair(&hub);
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    // Anyone may deliver (sealed sender), but only well-formed envelopes and with the user's devices named.
    let client = reqwest::blocking::Client::new();
    let without_devices =
        client.post(format!("{}{}", hub.url(), api::deliver_path(&[1; 16]))).body(vec![1, 2, 3]).send().unwrap();
    assert_eq!(without_devices.status().as_u16(), api::status::BAD_REQUEST);
    assert_eq!(deliver_raw(&hub.url(), b.user_id(), vec![1, 2, 3]), api::status::BAD_REQUEST);
    let garbage: Vec<u8> = (0..300).map(|i| (i * 7) as u8).collect();
    assert_eq!(deliver_raw(&hub.url(), b.user_id(), yo_wire::envelope::seal(&garbage).unwrap()), api::status::ACCEPTED);
    assert_eq!(b.sync(0).unwrap(), vec![]);
    assert_eq!(b.dropped(), 1);
    a.send_yo(&pb).unwrap();
    assert_eq!(yos_from(&b.sync(0).unwrap(), &pa), 1);
    assert_eq!(
        deliver_raw(&hub.url(), &"00".repeat(16), yo_wire::envelope::seal(b"x").unwrap()),
        api::status::NOT_FOUND
    );
}

#[test]
fn a_blocked_number_is_not_heard() {
    let hub = TestHub::start();
    let (mut a, mut b, pa, pb) = pair(&hub);
    b.set_blocked(&user_of(&pa), true).unwrap();
    a.send_yo(&pb).unwrap();
    let events = b.sync(0).unwrap();
    assert_eq!(yos_from(&events, &pa), 0);
    assert!(!events.iter().any(|e| matches!(e, Event::NewChat { .. })));
    assert_eq!(b.send_yo(&pa), Err(Error::Blocked));
}

#[test]
fn the_state_survives_a_restart_of_the_app() {
    let hub = TestHub::start();
    let (mut a, mut b, pa, pb) = pair(&hub);
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    let key = [42u8; 32];
    let blob = b.export(&key);
    assert!(Client::restore(Http::new(hub.url()), &blob, &[0; 32]).is_err());
    drop(b);
    let mut b = Client::restore(Http::new(hub.url()), &blob, &key).unwrap();
    b.send_yo(&pa).unwrap();
    assert_eq!(yos_from(&a.sync(0).unwrap(), &pb), 1);
    a.send_yo(&pb).unwrap();
    assert_eq!(yos_from(&b.sync(0).unwrap(), &pa), 1);
}

#[test]
fn the_outbox_survives_an_outage_of_the_hub() {
    let dir = std::env::temp_dir().join(format!("yo-outage-{}", random_u64()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut config = yo_hub::config::Config::for_tests();
    config.db_path = dir.join("hub.sqlite").to_string_lossy().into_owned();
    let hub = TestHub::start_with(config.clone(), "127.0.0.1:0");
    let addr = hub.addr.to_string();
    let (ha, hb) = (Http::new(hub.url()), Http::new(hub.url()));
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (phone(n), phone(n + 1));
    let mut a = register(ha.clone(), &pa, None);
    let mut b = register(hb.clone(), &pb, None);
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    hub.stop();
    // The hub is down: the message waits in the outbox, nothing is lost.
    a.send_yo(&pb).unwrap();
    assert_eq!(a.pending(), 1);
    assert!(matches!(b.sync(0), Err(Error::Network(_))));
    let hub = TestHub::start_with(config, &addr);
    a.sync(0).unwrap();
    assert_eq!(a.pending(), 0);
    assert_eq!(yos_from(&b.sync(0).unwrap(), &pa), 1);
    drop(hub);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_hub_survives_garbage_on_every_route() {
    let hub = TestHub::start();
    let client = reqwest::blocking::Client::new();
    let routes = [
        ("POST", api::VERIFY_START.to_owned()),
        ("POST", api::VERIFY_CHECK.to_owned()),
        ("PUT", api::KEY_PACKAGES.to_owned()),
        ("POST", api::LOOKUP.to_owned()),
        ("PUT", api::PUSH.to_owned()),
        ("GET", api::INBOX.to_owned()),
        ("POST", api::INBOX_ACK.to_owned()),
        ("POST", api::deliver_path(&[1; 16])),
        ("POST", api::GROUPS.to_owned()),
        ("POST", api::commit_path(&[1; 32])),
        ("POST", "/v1/inbox/!!".to_owned()),
        ("POST", "/v1/groups/x/commit".to_owned()),
        ("POST", api::REGISTER.to_owned()),
        ("PUT", api::BACKUP.to_owned()),
        ("GET", api::backup_path(&[1; 16])),
        ("GET", "/v1/backup/!!".to_owned()),
    ];
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    for (method, path) in routes {
        for len in [0usize, 1, 7, 64, 1024, 5000] {
            let body: Vec<u8> = (0..len)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    seed as u8
                })
                .collect();
            let status = client
                .request(method.parse().unwrap(), format!("{}{}", hub.url(), path))
                .header("yo-device", "AAAAAAAAAAAAAAAAAAAAAA")
                .header("yo-time", "1")
                .header("yo-signature", "x")
                .body(body)
                .send()
                .unwrap()
                .status()
                .as_u16();
            assert!(status < 500, "{method} {path} {len} bytes: {status}");
        }
    }
}

#[test]
fn registration_needs_the_code() {
    let hub = TestHub::start();
    let http = Http::new(hub.url());
    let p = phone(random_u64() % 1_000_000_000);
    Client::request_code(http.as_ref(), &p).unwrap();
    assert_eq!(
        Client::register(http, &p, "123456", None, Push::none()).err(),
        Some(Error::Refused(api::status::FORBIDDEN))
    );
}

#[test]
fn usernames_are_claimed_found_and_given_up() {
    let hub = TestHub::start();
    let (mut a, mut b, pa, _) = pair(&hub);
    let name = format!("ALICE{}", random_u64() % 100_000);
    a.set_username(Some(&name.to_lowercase())).unwrap();
    assert_eq!(a.username(), Some(name.as_str()));
    // B finds A by the username, typed any way.
    b.send_yo(&format!("@{}", name.to_lowercase())).unwrap();
    let events = a.sync(0).unwrap();
    assert_eq!(yos_from(&events, &format!("+{}", b.phone())), 1);
    assert!(b.contacts().iter().any(|c| c.user == user_of(&pa) && c.handles.contains(&name)));
    // Taken, reserved, malformed.
    assert_eq!(b.set_username(Some(&name)), Err(Error::Refused(api::status::CONFLICT)));
    assert!(b.set_username(Some("admin")).is_err());
    assert!(b.set_username(Some("x")).is_err());
    // Given up, it can be someone else's.
    a.set_username(None).unwrap();
    b.set_username(Some(&name)).unwrap();
    assert_eq!(b.username(), Some(name.as_str()));
}

#[test]
fn a_username_is_all_it_takes() {
    let hub = TestHub::start();
    let (mut a, _, pa, _) = pair(&hub);
    let name = format!("LEO{}", random_u64() % 100_000);
    let mut t = by_name(&hub, &name, None).unwrap();
    assert_eq!(t.username(), Some(name.as_str()));
    assert_eq!(t.phone(), "");
    t.send_yo(&pa).unwrap();
    assert_eq!(yos_from_user(&a.sync(0).unwrap(), t.user_id()), 1);
    a.send_yo(&name).unwrap();
    assert_eq!(yos_from_user(&t.sync(0).unwrap(), a.user_id()), 1);
    // The name is taken now, also as a name that only looks like it (spec 3.3).
    assert_eq!(by_name(&hub, &name, None).err(), Some(Error::Refused(api::status::CONFLICT)));
    let lookalike = name.replacen('O', "0", 1);
    assert_eq!(by_name(&hub, &lookalike, None).err(), Some(Error::Refused(api::status::CONFLICT)));
}

/// Registers by username, with a new account key or the given one.
fn by_name(hub: &TestHub, name: &str, account: Option<[u8; 32]>) -> Result<Client, Error> {
    Client::register_username(Http::new(hub.url()), name, account, Some("TESTER".into()), Push::none())
}

fn backup(locator: Vec<u8>) -> api::Backup {
    api::Backup::new(vec![1; 60], vec![2; 60], vec![3; 500], locator)
}

#[test]
fn a_new_phone_restores_a_username_with_its_account_key() {
    let hub = TestHub::start();
    let (mut a, _, pa, _) = pair(&hub);
    let name = format!("MIA{}", random_u64() % 100_000);
    let mut old = by_name(&hub, &name, None).unwrap();
    old.send_yo(&pa).unwrap();
    a.sync(0).unwrap();
    a.set_verified(old.user_id(), true).unwrap();
    // A new phone with the account key from the backup: the same user, the old phone replaced.
    let mut new = by_name(&hub, &name, Some(old.account_secret())).unwrap();
    assert_eq!(new.user_id(), old.user_id());
    assert_eq!(new.username(), Some(name.as_str()));
    assert_eq!(old.sync(0), Err(Error::Replaced));
    // Its contacts see nothing: the key is the same, the verification holds.
    let events = a.send_yo(&name).unwrap();
    assert!(!events.iter().any(|e| matches!(e, Event::IdentityChanged { .. })), "{events:?}");
    assert!(a.contacts()[0].verified);
    assert_eq!(yos_from_user(&new.sync(0).unwrap(), a.user_id()), 1);
    // Any other key: the name is taken.
    assert_eq!(by_name(&hub, &name, None).err(), Some(Error::Refused(api::status::CONFLICT)));
    // The key is the user's alone: it takes no other name.
    let other = format!("NOT{}", random_u64() % 100_000);
    assert_eq!(by_name(&hub, &other, Some(old.account_secret())).err(), Some(Error::Refused(api::status::CONFLICT)));
}

#[test]
fn a_backup_is_found_by_user_id_and_by_locator() {
    let hub = TestHub::start();
    let http = Http::new(hub.url());
    let mut t = by_name(&hub, &format!("SAFE{}", random_u64() % 100_000), None).unwrap();
    let user = yo_tests::user_id(t.user_id());
    assert_eq!(Client::fetch_backup(http.as_ref(), &user), Ok(None));
    let first = backup(vec![9; 16]);
    t.put_backup(&first).unwrap();
    assert_eq!(Client::fetch_backup(http.as_ref(), &user), Ok(Some(first.clone())));
    assert_eq!(Client::fetch_backup(http.as_ref(), &[9; 16]), Ok(Some(first)));
    // A new recovery code: the old locator finds nothing.
    let second = backup(vec![8; 16]);
    t.put_backup(&second).unwrap();
    assert_eq!(Client::fetch_backup(http.as_ref(), &[9; 16]), Ok(None));
    assert_eq!(Client::fetch_backup(http.as_ref(), &[8; 16]), Ok(Some(second.clone())));
    assert_eq!(Client::fetch_backup(http.as_ref(), &user), Ok(Some(second)));
    // Another account cannot take the locator.
    let mut u = by_name(&hub, &format!("OTHER{}", random_u64() % 100_000), None).unwrap();
    assert_eq!(u.put_backup(&backup(vec![8; 16])), Err(Error::Refused(api::status::CONFLICT)));
    // Malformed records are refused, by the core and by the hub.
    let bad = api::Backup { version: 2, ..backup(Vec::new()) };
    assert!(t.put_backup(&bad).is_err());
    for body in [yo_wire::encode(&bad), yo_wire::encode(&backup(vec![1; 15])), vec![1, 2, 3], Vec::new()] {
        assert_eq!(t.signed_for_tests("PUT", api::BACKUP, &body).unwrap().status, api::status::BAD_REQUEST);
    }
    assert_eq!(Client::fetch_backup(http.as_ref(), &user), Ok(Some(backup(vec![8; 16]))), "nothing changed");
    assert_eq!(Client::fetch_backup(http.as_ref(), &[1; 15]), Err(Error::Protocol("backup id")));
}

#[test]
fn names_not_secured_are_released_after_ninety_days() {
    const DAY: u64 = 24 * 3600 * 1000;
    let hub = TestHub::start();
    let (secured_name, idle_name) =
        (format!("KEPT{}", random_u64() % 100_000), format!("GONE{}", random_u64() % 100_000));
    let mut secured = by_name(&hub, &secured_name, None).unwrap();
    let mut idle = by_name(&hub, &idle_name, None).unwrap();
    secured.put_backup(&backup(Vec::new())).unwrap();
    let now = yo_hub::now_ms();
    hub.hub.prune(now + 89 * DAY).unwrap();
    idle.sync(0).unwrap();
    hub.hub.prune(now + 91 * DAY).unwrap();
    assert_eq!(idle.sync(0), Err(Error::Replaced), "the device learns that its account is gone");
    secured.sync(0).unwrap();
    // The name is free again; the secured one is kept.
    by_name(&hub, &idle_name, None).unwrap();
    assert_eq!(by_name(&hub, &secured_name, None).err(), Some(Error::Refused(api::status::CONFLICT)));
}

#[test]
fn registration_and_backups_are_limited_per_address() {
    let mut config = yo_hub::config::Config::for_tests();
    config.limits.register_per_ip_hour = 2;
    config.limits.backup_per_ip_hour = 2;
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let n = random_u64() % 100_000;
    by_name(&hub, &format!("ONE{n}"), None).unwrap();
    by_name(&hub, &format!("TWO{n}"), None).unwrap();
    assert_eq!(by_name(&hub, &format!("THREE{n}"), None).err(), Some(Error::Refused(api::status::RATE_LIMITED)));
    let http = Http::new(hub.url());
    for _ in 0..2 {
        assert_eq!(Client::fetch_backup(http.as_ref(), &[5; 16]), Ok(None));
    }
    assert_eq!(Client::fetch_backup(http.as_ref(), &[5; 16]), Err(Error::Refused(api::status::RATE_LIMITED)));
}

#[test]
fn an_account_key_belongs_to_one_user_forever() {
    let hub = TestHub::start();
    let (a, _, _, _) = pair(&hub);
    // Another number cannot take A's account key, not even after A replaces it.
    let other = phone(random_u64() % 1_000_000_000);
    let http = Http::new(hub.url());
    Client::request_code(http.as_ref(), &other).unwrap();
    assert_eq!(
        Client::register_keeping_key(http, &other, "000000", a.account_secret()).err(),
        Some(Error::Refused(api::status::CONFLICT))
    );
}

#[test]
fn a_copied_registration_registers_no_one() {
    let hub = TestHub::start();
    let http = Http::new(hub.url());
    let number = phone(random_u64() % 1_000_000_000);
    register(http.clone(), &number, None);
    let (_, _, body) =
        http.sent.lock().unwrap().iter().find(|(_, path, _)| path == api::VERIFY_CHECK).cloned().unwrap();
    let mut check: api::VerifyCheck = yo_wire::decode(&body).unwrap();
    // Someone copies certificate and KeyPackages and puts in their own auth key: the proof no longer fits.
    check.device.auth_key = [7; 32];
    check.device.device_id = [7; 16];
    let replay = reqwest::blocking::Client::new()
        .post(format!("{}{}", hub.url(), api::VERIFY_CHECK))
        .body(yo_wire::encode(&check))
        .send()
        .unwrap();
    assert_eq!(replay.status().as_u16(), api::status::BAD_REQUEST);
}

fn yos_from_user(events: &[Event], from: &str) -> usize {
    events.iter().filter(|e| matches!(e, Event::Message { user, content: Content::Yo, .. } if user == from)).count()
}
