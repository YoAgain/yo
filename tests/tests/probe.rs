//! Probes stay silent; ordinary echo Yos remain visible.
use std::time::Duration;
use yo_core::{Client, Content, Event, Push};
use yo_tests::{register, Http, TestHub};

fn storage(client: &Client) -> String {
    let state = client.saved().state;
    serde_json::to_string(&state.storage).unwrap()
}

#[test]
fn silent_probe_measures_a_real_yo_and_preserves_other_events_and_storage() {
    let hub = TestHub::start();
    let mut alice = register(Http::new(hub.url()), "ALICE", None);
    let mut ping = register(Http::new(hub.url()), "PING", None);
    let mut bob = register(Http::new(hub.url()), "BOB", None);
    alice.maintenance().unwrap();
    let alice_id = alice.user_id().to_owned();
    let responder = std::thread::spawn(move || loop {
        ping.sync(0).unwrap();
        if let Some((user, token)) = ping.take_probe_requests().pop() {
            bob.send_yo(&alice_id).unwrap();
            ping.echo_probe(&user, token).unwrap();
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    });
    let ms = alice.round_trip_probe("PING", 3000).unwrap().expect("echo");
    assert!(ms < 3000);
    responder.join().unwrap();
    let events = alice.sync(0).unwrap();
    assert!(
        events.iter().any(|e| matches!(e, Event::Message { content: Content::Yo, .. })),
        "ordinary Yo must survive"
    );
    assert!(alice.contacts().iter().all(|c| c.handles.iter().all(|h| h != "PING")));
    assert!(alice.friends().is_empty());
    // Bob's ordinary Yo legitimately stores his key. The service never does.
    let state = alice.saved().state;
    assert!(state.contacts.values().filter(|c| c.probe_service).all(|c| !c.chat));
    for (user, c) in &state.contacts {
        if c.probe_service {
            assert!(!state.storage.keys.contains_key(user));
            assert!(!state.storage.friends.contains_key(user));
            assert!(events
                .iter()
                .all(|e| !matches!(e, Event::Message { user: u, .. } | Event::NewChat { user: u } if u == user)));
        }
    }
}

#[test]
fn timed_out_and_unsolicited_answers_stay_silent_after_restore() {
    let hub = TestHub::start();
    let mut alice = register(Http::new(hub.url()), "ALICE", None);
    let mut ping = register(Http::new(hub.url()), "PING", None);
    alice.maintenance().unwrap();
    let before = storage(&alice);
    assert_eq!(alice.round_trip_probe("PING", 30).unwrap(), None);
    assert!(alice.contacts().is_empty());
    ping.sync(0).unwrap();
    let (user, token) = ping.take_probe_requests().pop().unwrap();
    ping.echo_probe(&user, token).unwrap();
    // Reopening has no active waiter. The late answer must still be consumed silently.
    let mut restored = Client::restore(Http::new(hub.url()), &alice.saved()).unwrap();
    restored.set_trust_roots(vec![yo_tests::trust_root()]);
    assert!(restored.sync(0).unwrap().is_empty());
    assert!(restored.contacts().is_empty());
    assert!(restored.friends().is_empty());
    assert_eq!(storage(&restored), before);
    // A different device that never initiated the probe learns only protocol routing.
    let mut other =
        Client::register_username(Http::new(hub.url()), "ALICE", Some(alice.account_secret()), None, Push::none())
            .unwrap();
    other.set_trust_roots(vec![yo_tests::trust_root()]);
    let other_before = storage(&other);
    ping.send_yo(other.user_id()).unwrap();
    assert!(other.sync(0).unwrap().is_empty());
    assert!(other.contacts().is_empty());
    assert!(other.friends().is_empty());
    assert_eq!(storage(&other), other_before);
}

#[test]
fn repeated_probes_work_with_requests_closed_and_never_change_synced_storage() {
    let hub = TestHub::start();
    let mut alice = register(Http::new(hub.url()), "ALICE", None);
    alice.set_requests_open(false).unwrap();
    alice.maintenance().unwrap();
    let before = storage(&alice);
    let mut ping = register(Http::new(hub.url()), "PING", None);
    let user = alice.user_id().to_owned();
    let responder = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut answered = 0;
        while answered < 3 {
            assert!(std::time::Instant::now() < deadline);
            ping.sync(0).unwrap();
            for (sender, token) in ping.take_probe_requests() {
                assert_eq!(sender, user);
                ping.echo_probe(&sender, token).unwrap();
                answered += 1;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    for _ in 0..3 {
        assert!(alice.round_trip_probe("PING", 3000).unwrap().is_some());
        assert!(alice.sync(0).unwrap().is_empty());
        assert!(alice.contacts().is_empty());
        assert!(alice.friends().is_empty());
        assert_eq!(storage(&alice), before);
    }
    responder.join().unwrap();
}

#[test]
fn failed_probe_is_not_retried_by_a_later_sync() {
    let hub = TestHub::start();
    let http = Http::new(hub.url());
    let mut alice = register(http.clone(), "ALICE", None);
    let mut ping = register(Http::new(hub.url()), "PING", None);
    let path = yoip::net::api::deliver_path(&yo_tests::user_id(ping.user_id()));
    http.fault(&path, yo_tests::Fault::Answer(503));
    assert_eq!(alice.round_trip_probe("PING", 30).unwrap(), None);
    let state = alice.saved().state;
    assert!(state.outbox.iter().all(|item| item.user != ping.user_id()));
    alice.sync(0).unwrap();
    alice.maintenance().unwrap();
    assert!(ping.sync(0).unwrap().is_empty());
    assert!(alice.contacts().is_empty());
}

#[test]
fn warm_probe_reuses_session_without_catch_up() {
    let hub = TestHub::start();
    let http = Http::new(hub.url());
    let mut alice = register(http.clone(), "ALICE", None);
    let mut ping = register(Http::new(hub.url()), "PING", None);
    alice.maintenance().unwrap();
    let user = alice.user_id().to_owned();
    let responder = std::thread::spawn(move || {
        for _ in 0..2 {
            loop {
                ping.sync(10).unwrap();
                if let Some((sender, token)) = ping.take_probe_requests().pop() {
                    assert_eq!(sender, user);
                    ping.echo_probe(&sender, token).unwrap();
                    break;
                }
            }
        }
    });
    assert!(alice.round_trip_probe("PING", 3000).unwrap().is_some());
    // Drain the first answer's ack outside the measurement, as an ordinary sync would.
    alice.sync(0).unwrap();
    http.sent.lock().unwrap().clear();
    let ms = alice.round_trip_probe("PING", 3000).unwrap().unwrap();
    responder.join().unwrap();
    let sent = http.sent.lock().unwrap();
    let routes: Vec<_> = sent.iter().map(|(method, path, _)| (method.as_str(), path.as_str())).collect();
    eprintln!("warm probe: {} HTTP requests, {ms} ms: {routes:?}", routes.len());
    assert_eq!(routes.len(), 2, "one sealed POST and one long-poll GET");
    assert!(sent
        .iter()
        .filter(|(method, _, _)| method == "POST")
        .all(|(_, _, body)| body[0] == yoip::protocol::envelope::SEALED));
    assert!(routes.contains(&("GET", "/v1/inbox?wait=3")));
    assert!(!routes.iter().any(|(_, path)| path.contains("lookup") || path.starts_with("/v1/inbox?wait=0")));
}

#[test]
fn echo_answers_before_inbox_maintenance_without_catch_up() {
    let hub = TestHub::start();
    let mut alice = register(Http::new(hub.url()), "ALICE", None);
    let http = Http::new(hub.url());
    let mut ping = register(http.clone(), "PING", None);
    alice.send_yo("PING").unwrap();
    assert!(ping.sync(10).unwrap().iter().any(|e| matches!(e, Event::Message { content: Content::Yo, .. })));
    http.sent.lock().unwrap().clear();
    ping.echo_yo(alice.user_id()).unwrap();
    let sent = http.sent.lock().unwrap();
    assert!(!sent
        .iter()
        .any(|(_, path, _)| path.contains("lookup") || path.contains("wait=") || path.ends_with("/ack")));
    assert!(
        alice.sync(0).unwrap().iter().any(|e| matches!(e, Event::Message { content: Content::Yo, .. })),
        "PING echoes are visible"
    );
}

#[test]
fn normal_warm_request_counts() {
    let hub = TestHub::start();
    let http = Http::new(hub.url());
    let mut alice = register(http.clone(), "ALICE", None);
    let bh = Http::new(hub.url());
    let mut bob = register(bh.clone(), "BOB", None);
    alice.send_yo("BOB").unwrap();
    bob.sync(0).unwrap();
    bob.send_yo(alice.user_id()).unwrap();
    alice.sync(0).unwrap();
    alice.after_sync().unwrap();
    bob.sync(0).unwrap();
    bob.after_sync().unwrap();
    for _ in 0..2 {
        alice.sync(0).unwrap();
        alice.after_sync().unwrap();
        bob.sync(0).unwrap();
        bob.after_sync().unwrap();
    }
    http.sent.lock().unwrap().clear();
    alice.send_yo(bob.user_id()).unwrap();
    let sent = http.sent.lock().unwrap().clone();
    eprintln!(
        "normal warm send: {} requests: {:?}",
        sent.len(),
        sent.iter().map(|(m, p, _)| (m, p)).collect::<Vec<_>>()
    );
    assert_eq!(sent.len(), 1, "warm send is exactly one POST");
    assert_eq!(sent[0].0, "POST");
    assert_eq!(sent[0].2[0], yoip::protocol::envelope::SEALED, "the single request uses sealed sender");
    bh.sent.lock().unwrap().clear();
    bob.sync(25).unwrap();
    let received = bh.sent.lock().unwrap().clone();
    assert_eq!(received.len(), 1, "receive before housekeeping is one long-poll GET");
    eprintln!(
        "normal receive: {} requests: {:?}",
        received.len(),
        received.iter().map(|(m, p, _)| (m, p)).collect::<Vec<_>>()
    );
}

#[test]
fn added_pingtest_shows_normal_yos_but_only_the_matched_probe_is_consumed() {
    let hub = TestHub::start();
    let mut alice = register(Http::new(hub.url()), "ALICE", None);
    let mut ping = register(Http::new(hub.url()), "PING", None);
    // A previous silent probe must not prevent subsequently adding PING.
    assert_eq!(alice.round_trip_probe("PING", 10).unwrap(), None);
    ping.sync(0).unwrap();
    let (user, late) = ping.take_probe_requests().pop().unwrap();
    alice.set_friend(ping.user_id(), Some("PING".into()), None).unwrap();
    alice.send_yo("PING").unwrap();
    assert_eq!(alice.friends().len(), 1);
    assert_eq!(alice.contacts().len(), 1);
    let probe = alice.round_trip_probe_begin("PING", 3000).unwrap();
    let received = ping.sync(0).unwrap();
    assert_eq!(received.iter().filter(|e| matches!(e, Event::Message { content: Content::Yo, .. })).count(), 1);
    let (_, token) = ping.take_probe_requests().pop().unwrap();
    // An old reply must not complete the new measurement, even if it arrives first.
    ping.echo_probe(&user, late).unwrap();
    let mut reply = probe.receive(3000).unwrap();
    assert!(!alice.round_trip_probe_poll(&mut reply).unwrap());
    let probe = alice.round_trip_probe_wait(&reply, 3000).unwrap();
    ping.echo_yo(&user).unwrap();
    ping.echo_probe(&user, token).unwrap();
    let mut reply = probe.receive(3000).unwrap();
    if !alice.round_trip_probe_poll(&mut reply).unwrap() {
        let probe = alice.round_trip_probe_wait(&reply, 3000).unwrap();
        let mut reply = probe.receive(3000).unwrap();
        assert!(alice.round_trip_probe_poll(&mut reply).unwrap());
    }
    let events = alice.saved_for_app().unwrap();
    assert_eq!(events.iter().filter(|e| matches!(e, Event::Message { content: Content::Yo, .. })).count(), 1);
    assert_eq!(alice.friends().len(), 1);
}

#[test]
fn empty_inbox_passes_do_not_save_full_state() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    struct Saves(AtomicUsize);
    impl yo_core::Store for Saves {
        fn save_state(&self, _: &yo_core::state::State, _: yo_core::signal::Entries) -> Result<(), String> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }
    let hub = TestHub::start();
    let mut alice = register(Http::new(hub.url()), "ALICE", None);
    alice.maintenance().unwrap();
    let saves = Arc::new(Saves(AtomicUsize::new(0)));
    alice.persist_to(saves.clone(), [42; 32]).unwrap();
    saves.0.store(0, Ordering::Relaxed);
    for _ in 0..3 {
        assert!(alice.sync(0).unwrap().is_empty());
    }
    assert_eq!(saves.0.load(Ordering::Relaxed), 0);
}

#[test]
fn failed_probe_keeps_an_added_pingtests_ordinary_outbox() {
    let hub = TestHub::start();
    let http = Http::new(hub.url());
    let mut alice = register(http.clone(), "ALICE", None);
    let mut ping = register(Http::new(hub.url()), "PING", None);
    alice.send_yo("PING").unwrap();
    ping.sync(0).unwrap();
    ping.echo_yo(alice.user_id()).unwrap();
    alice.sync(0).unwrap();
    alice.after_sync().unwrap();
    let path = yoip::net::api::deliver_path(&yo_tests::user_id(ping.user_id()));
    // Leave a normal Yo queued, then refuse every attempted delivery during a probe.
    for _ in 0..12 {
        http.fault(&path, yo_tests::Fault::Answer(503));
    }
    alice.send_yo("PING").unwrap();
    assert_eq!(alice.round_trip_probe("PING", 10).unwrap(), None);
    let state = alice.saved().state;
    let contents: Vec<_> = state
        .outbox
        .iter()
        .filter(|item| item.user == ping.user_id())
        .map(|item| {
            let plain: yoip::protocol::envelope::Plain = yoip::decode(&item.plain).unwrap();
            yoip::protocol::content::Message::decode(plain.content.as_slice()).unwrap().content
        })
        .collect();
    assert!(contents.contains(&Content::Yo));
    assert!(!contents.iter().any(|c| matches!(c, Content::Probe(_))));
    http.faults.lock().unwrap().clear();
    alice.sync(0).unwrap();
    assert_eq!(
        ping.sync(0).unwrap().iter().filter(|e| matches!(e, Event::Message { content: Content::Yo, .. })).count(),
        1
    );
}

#[test]
fn a_probe_to_an_added_pingtest_does_not_change_account_storage() {
    let hub = TestHub::start();
    let mut alice = register(Http::new(hub.url()), "ALICE", None);
    let mut ping = register(Http::new(hub.url()), "PING", None);
    alice.maintenance().unwrap();
    ping.maintenance().unwrap();
    alice.send_yo("PING").unwrap();
    ping.sync(0).unwrap();
    ping.echo_yo(alice.user_id()).unwrap();
    alice.set_friend(ping.user_id(), Some("PING".into()), None).unwrap();
    for _ in 0..3 {
        alice.sync(0).unwrap();
        alice.after_sync().unwrap();
        ping.sync(0).unwrap();
        ping.after_sync().unwrap();
    }
    let before = storage(&alice);
    let responder = std::thread::spawn(move || loop {
        ping.sync(3).unwrap();
        if let Some((user, token)) = ping.take_probe_requests().pop() {
            ping.echo_probe(&user, token).unwrap();
            break;
        }
    });
    assert!(alice.round_trip_probe("PING", 3000).unwrap().is_some());
    responder.join().unwrap();
    assert!(alice.saved_for_app().unwrap().is_empty());
    assert_eq!(storage(&alice), before);
    assert_eq!(alice.friends().len(), 1);
}

/// PING in a group is a member like any: its group Yos reach a member who never added it, with no 1:1 chat made.
#[test]
fn pingtest_in_a_group_reaches_members_who_never_added_it() {
    let hub = TestHub::start();
    let mut alice = register(Http::new(hub.url()), "ALICE", None);
    let mut ping = register(Http::new(hub.url()), "PING", None);
    let mut cleo = register(Http::new(hub.url()), "CLEO", None);
    let (id, _) = alice.create_group("ECHO", &[ping.user_id().to_owned(), cleo.user_id().to_owned()]).unwrap();
    for x in [&mut ping, &mut cleo] {
        for _ in 0..3 {
            x.sync(0).unwrap();
        }
    }
    ping.send_group(&id, Content::Yo).unwrap();
    let events: Vec<Event> = (0..3).flat_map(|_| cleo.sync(0).unwrap()).collect();
    let from_ping = |e: &Event| matches!(e, Event::GroupMessage { group, user, content: Content::Yo, .. } if *group == id && user == ping.user_id());
    assert_eq!(events.iter().filter(|e| from_ping(e)).count(), 1, "{events:?}");
    assert!(!events.iter().any(|e| matches!(e, Event::NewChat { .. } | Event::Message { .. })));
    assert!(cleo.contacts().iter().all(|c| c.handles.iter().all(|h| h != "PING")));
    // Cleo knows it by its name: still the probe service to her, not a chat.
    let state = cleo.saved().state;
    assert!(state.contacts.get(ping.user_id()).is_some_and(|c| c.probe_service && !c.chat));
}
