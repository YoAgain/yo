//! Scenarios: a real hub, real clients over HTTP, the protocol end to end (spec/v1.md).

use std::sync::Arc;
use yo_core::{Client, Content, Error, Event, Push};
use yo_tests::{phone, random_u64, register, user_of, Http, TestHub};
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
    assert_eq!(a.dropped() + b.dropped(), 0);
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
fn a_new_phone_without_passkey_is_a_new_identity() {
    let hub = TestHub::start();
    let (mut a, mut b, pa, pb) = pair(&hub);
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    // B registers again: a new device, a new account key.
    let mut b2 = register(Http::new(hub.url()), &pb, Some("BOB"));
    assert_eq!(b2.user_id(), b.user_id(), "a new phone keeps the user id");
    assert_eq!(b.sync(0), Err(Error::Replaced));
    let events = a.send_yo(&pb).unwrap();
    assert!(events.contains(&Event::IdentityChanged { user: digits(&pb) }), "{events:?}");
    let events = b2.sync(0).unwrap();
    assert!(events.contains(&Event::NewChat { user: digits(&pa) }));
    assert_eq!(yos_from(&events, &pa), 1);
    b2.send_yo(&pa).unwrap();
    assert_eq!(yos_from(&a.sync(0).unwrap(), &pb), 1);
    // The old chat is closed: nothing is sent there, what it still holds is read for 31 days, then it goes.
    assert_eq!(a.chats().len(), 2);
    let later = yo_hub::now_ms() + 32 * 24 * 3600 * 1000;
    a.set_clock(Arc::new(move || later));
    a.maintenance().unwrap();
    assert_eq!(a.chats().len(), 1, "the closed chat is dropped after the envelope TTL");
    b2.send_yo(&pa).unwrap();
    assert_eq!(yos_from(&a.sync(0).unwrap(), &pb), 1);
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
    let (_, group, epoch) = a.chats()[0].clone();
    let client = reqwest::blocking::Client::new();
    let post = |body: Vec<u8>| {
        client.post(format!("{}{}", hub.url(), api::commit_path(&group))).body(body).send().unwrap().status().as_u16()
    };
    let forged = api::Commit { epoch, token: [9; 32], next_token_hash: [9; 32] };
    assert_eq!(post(yo_wire::encode(&forged)), api::status::FORBIDDEN);
    let stale = api::Commit { epoch: epoch - 1, token: [9; 32], next_token_hash: [9; 32] };
    assert_eq!(post(yo_wire::encode(&stale)), api::status::CONFLICT);
    b.send_yo(&pa).unwrap();
    assert_eq!(yos_from(&a.sync(0).unwrap(), &pb), 1);
}

#[test]
fn garbage_in_a_mailbox_is_dropped_and_harms_nothing() {
    let hub = TestHub::start();
    let (mut a, mut b, pa, pb) = pair(&hub);
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    let client = reqwest::blocking::Client::new();
    let deliver = |body: Vec<u8>| {
        client
            .post(format!("{}{}", hub.url(), api::deliver_path(&b.mailbox())))
            .body(body)
            .send()
            .unwrap()
            .status()
            .as_u16()
    };
    // Anyone may deliver (sealed sender), but only well-formed envelopes.
    assert_eq!(deliver(vec![1, 2, 3]), api::status::BAD_REQUEST);
    let garbage: Vec<u8> = (0..300).map(|i| (i * 7) as u8).collect();
    assert_eq!(deliver(yo_wire::envelope::seal(&garbage).unwrap()), api::status::ACCEPTED);
    assert_eq!(b.sync(0).unwrap(), vec![]);
    assert_eq!(b.dropped(), 1);
    a.send_yo(&pb).unwrap();
    assert_eq!(yos_from(&b.sync(0).unwrap(), &pa), 1);
    let unknown = client
        .post(format!("{}{}", hub.url(), api::deliver_path(&[0; 16])))
        .body(yo_wire::envelope::seal(b"x").unwrap())
        .send()
        .unwrap();
    assert_eq!(unknown.status().as_u16(), api::status::NOT_FOUND);
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
