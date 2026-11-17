//! Reserved service Yos stay out of app events and encrypted Account Sync storage.
use std::time::Duration;
use yo_core::{state, Client, Content, Event, Push};
use yo_tests::{register, Http, TestHub};

fn storage(client: &Client) -> String {
    let (state, _) = state::open(&client.export(&[42; 32]), &[42; 32]).unwrap();
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
        let events = ping.sync(0).unwrap();
        if events.iter().any(|e| matches!(e, Event::Message { content: Content::Yo, .. })) {
            bob.send_yo(&alice_id).unwrap();
            ping.send_yo(&alice_id).unwrap();
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
    let (state, _) = state::open(&alice.export(&[42; 32]), &[42; 32]).unwrap();
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
    assert!(ping.sync(0).unwrap().iter().any(|e| matches!(e, Event::Message { content: Content::Yo, .. })));
    ping.send_yo(alice.user_id()).unwrap();
    // Reopening has no active waiter. The late answer must still be consumed silently.
    let mut restored = Client::restore(Http::new(hub.url()), &alice.export(&[42; 32]), &[42; 32]).unwrap();
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
            for event in ping.sync(0).unwrap() {
                if matches!(event, Event::Message { content: Content::Yo, .. }) {
                    ping.send_yo(&user).unwrap();
                    answered += 1;
                }
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
    let path = yo_wire::api::deliver_path(&yo_tests::user_id(ping.user_id()));
    http.fault(&path, yo_tests::Fault::Answer(503));
    assert_eq!(alice.round_trip_probe("PING", 30).unwrap(), None);
    let (state, _) = state::open(&alice.export(&[42; 32]), &[42; 32]).unwrap();
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
                if ping.sync(10).unwrap().iter().any(|e| matches!(e, Event::Message { content: Content::Yo, .. })) {
                    ping.send_yo(&user).unwrap();
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
    assert!(routes.contains(&("GET", "/v1/inbox?wait=3")));
    assert!(!routes.iter().any(|(_, path)| path.contains("lookup") || *path == "/v1/inbox?wait=0"));
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
    assert!(alice.sync(0).unwrap().is_empty(), "PING's echo is silent");
}
