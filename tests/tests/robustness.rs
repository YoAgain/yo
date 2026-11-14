//! What goes wrong on the way: lost answers, crashes between steps, passing refusals, hostile senders.

use std::sync::{Arc, Mutex};
use yo_core::{Client, Content, Event, Store};
use yo_hub::config::Config;
use yo_tests::{deliver_raw, new_phone, random_u64, register, user_of, username, Fault, Http, TestHub};
use yo_wire::api;

fn yos(events: &[Event]) -> usize {
    events.iter().filter(|e| matches!(e, Event::Message { content: Content::Yo, .. })).count()
}

/// A chat where B has joined and both updated once: the steady state.
fn chat(hub: &TestHub) -> (Client, Client, Arc<Http>, Arc<Http>, String, String) {
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (username(n), username(n + 1));
    let (ha, hb) = (Http::new(hub.url()), Http::new(hub.url()));
    let mut a = register(ha.clone(), &pa, None);
    let mut b = register(hb.clone(), &pb, None);
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    b.send_yo(&pa).unwrap();
    a.sync(0).unwrap();
    (a, b, ha, hb, pa, pb)
}

#[test]
fn a_commit_whose_answer_was_lost_is_settled_by_repeating_it() {
    let hub = TestHub::start();
    let (mut a, mut b, ha, _, pa, pb) = chat(&hub);
    let epoch = a.chats()[0].2;
    let later = yo_hub::now_ms() + yo_core::UPDATE_AFTER_MS + 1;
    a.set_clock(Arc::new(move || later));
    // The hub accepts A's update; A never hears it.
    ha.fault("/commit", Fault::LoseAnswer);
    assert!(matches!(a.maintenance(), Err(yo_core::Error::Network(_))));
    // The next send repeats the commit, the hub confirms it, the group moves on, the Yo follows.
    a.send_yo(&pb).unwrap();
    assert_eq!(yos(&b.sync(0).unwrap()), 1);
    assert_eq!(a.chats()[0].2, epoch + 1);
    assert_eq!(b.chats()[0].2, epoch + 1);
    b.send_yo(&pa).unwrap();
    assert_eq!(yos(&a.sync(0).unwrap()), 1);
    assert_eq!(a.dropped() + b.dropped(), 0, "{:?} {:?}", a.drop_reasons(), b.drop_reasons());
}

#[test]
fn a_commit_lost_while_the_peer_won_is_dropped_cleanly() {
    let hub = TestHub::start();
    let (mut a, mut b, ha, _, pa, pb) = chat(&hub);
    let later = yo_hub::now_ms() + yo_core::UPDATE_AFTER_MS + 1;
    a.set_clock(Arc::new(move || later));
    b.set_clock(Arc::new(move || later));
    // A's commit never reaches the hub (refused before), then B wins the epoch.
    ha.fault("/commit", Fault::Answer(503));
    assert!(a.maintenance().is_err());
    b.maintenance().unwrap();
    // A learns of B's commit, drops its own, and the chat goes on.
    let mut events = a.sync(0).unwrap();
    events.extend(a.send_yo(&pb).unwrap());
    assert_eq!(yos(&b.sync(0).unwrap()), 1);
    b.send_yo(&pa).unwrap();
    events.extend(a.sync(0).unwrap());
    assert_eq!(yos(&events), 1);
    assert_eq!(a.chats()[0].2, b.chats()[0].2);
    assert_eq!(a.dropped() + b.dropped(), 0, "{:?} {:?}", a.drop_reasons(), b.drop_reasons());
}

#[derive(Default)]
struct Saved(Mutex<Vec<u8>>);

impl Store for Saved {
    fn save(&self, blob: Vec<u8>) {
        *self.0.lock().unwrap() = blob;
    }
}

#[test]
fn a_crash_after_processing_loses_nothing_and_repeats_nothing() {
    let hub = TestHub::start();
    let (mut a, mut b, _, hb, pa, pb) = chat(&hub);
    let saved = Arc::new(Saved::default());
    let key = [5u8; 32];
    b.persist_to(saved.clone(), key);
    for _ in 0..3 {
        a.send_yo(&pb).unwrap();
    }
    // B processes the three Yos and saves; the ack is lost; B is killed before anything else.
    hb.fault("/v1/inbox/ack", Fault::LoseAnswer);
    let first = b.sync(0).unwrap();
    assert_eq!(yos(&first), 3);
    // Killed right after the call: the app may not have the events yet, so they come once more after the restart.
    let blob = saved.0.lock().unwrap().clone();
    let mut again = Client::restore(Http::new(hub.url()), &blob, &key).unwrap();
    assert_eq!(yos(&again.sync(0).unwrap()), 3, "not acknowledged: at least once");
    // The app acknowledged them before it was killed: the hub delivers the envelopes again (the ack never came),
    // they are recognised, and nothing is shown twice.
    b.acknowledge_events();
    drop(again);
    drop(b);
    let blob = saved.0.lock().unwrap().clone();
    let mut b = Client::restore(Http::new(hub.url()), &blob, &key).unwrap();
    assert_eq!(yos(&b.sync(0).unwrap()), 0);
    a.send_yo(&pb).unwrap();
    assert_eq!(yos(&b.sync(0).unwrap()), 1);
    b.send_yo(&pa).unwrap();
    assert_eq!(yos(&a.sync(0).unwrap()), 1);
    assert_eq!(b.dropped(), 0, "{:?}", b.drop_reasons());
}

#[test]
fn a_passing_refusal_does_not_drop_a_welcome() {
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (username(n), username(n + 1));
    let hb = Http::new(hub.url());
    let mut a = register(Http::new(hub.url()), &pa, None);
    let mut b = register(hb.clone(), &pb, None);
    a.send_yo(&pb).unwrap();
    // B's check of A's identity hits a rate limit: the Welcome stays at the hub.
    hb.fault("/v1/lookup", Fault::Answer(429));
    assert!(b.sync(0).is_err());
    let events = b.sync(0).unwrap();
    assert!(events.contains(&Event::NewChat { user: user_of(&pa) }));
    assert_eq!(yos(&events), 1);
    assert_eq!(b.dropped(), 0, "{:?}", b.drop_reasons());
}

#[test]
fn a_full_mailbox_refuses_until_it_is_read_and_strangers_cannot_rate_limit_it() {
    let mut config = Config::for_tests();
    config.limits.max_pending_per_device = 5;
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let (mut a, mut b, _, _, pa, pb) = chat(&hub);
    let junk = yo_wire::envelope::seal(&[7; 100]).unwrap();
    // A stranger fills B's inbox with well-formed junk...
    let statuses: Vec<u16> = (0..6).map(|_| deliver_raw(&hub.url(), b.user_id(), junk.clone())).collect();
    assert_eq!(statuses, vec![202, 202, 202, 202, 202, 429]);
    // ... which holds A's Yo back only until B reads its mailbox.
    a.send_yo(&pb).unwrap();
    assert_eq!(a.pending(), 1);
    assert_eq!(yos(&b.sync(0).unwrap()), 0);
    a.sync(0).unwrap();
    assert_eq!(a.pending(), 0);
    assert_eq!(yos(&b.sync(0).unwrap()), 1);
    b.send_yo(&pa).unwrap();
    assert_eq!(yos(&a.sync(0).unwrap()), 1);
}

#[test]
fn the_hub_repeats_an_accepted_commit_and_nothing_else() {
    let hub = TestHub::start();
    let client = reqwest::blocking::Client::new();
    let group = [3u8; 32];
    let token = [1u8; 32];
    let create = api::GroupCreate { group_id: group, epoch: 1, token_hash: yo_wire::sha256(&token) };
    let created = client.post(format!("{}{}", hub.url(), api::GROUPS)).body(yo_wire::encode(&create)).send().unwrap();
    assert_eq!(created.status().as_u16(), 201);
    let post = |commit: &api::Commit| {
        client
            .post(format!("{}{}", hub.url(), api::commit_path(&group)))
            .body(yo_wire::encode(commit))
            .send()
            .unwrap()
            .status()
            .as_u16()
    };
    let envelope = yo_wire::envelope::seal(b"the commit").unwrap();
    let commit = api::Commit { epoch: 1, token, next_token_hash: [9; 32], envelope: envelope.clone().into() };
    assert_eq!(post(&commit), 200);
    assert_eq!(post(&commit), 200, "the same commit again");
    assert_eq!(
        post(&api::Commit { next_token_hash: [8; 32], ..commit.clone() }),
        409,
        "another commit on the old epoch"
    );
    // The hub kept it, for whoever missed it.
    let kept = client.get(format!("{}{}", hub.url(), api::commit_fetch_path(&group, 1))).send().unwrap();
    assert_eq!(kept.bytes().unwrap().to_vec(), envelope);
    let missing = client.get(format!("{}{}", hub.url(), api::commit_fetch_path(&group, 2))).send().unwrap();
    assert_eq!(missing.status().as_u16(), 404);
}

/// A commit fetched is `410` only where the group had one accepted that is kept no more: one never accepted (before
/// the group, or not yet) is `404`, which drops a made-up epoch and leaves the chat as it is (spec 4.9). A commit
/// repeated after its envelope is gone is still the one accepted (4.3).
#[test]
fn a_pruned_commit_is_gone_one_never_accepted_is_unknown_and_a_repeat_is_still_accepted() {
    let hub = TestHub::start();
    let client = reqwest::blocking::Client::new();
    let group = [4u8; 32];
    let tokens = [[1u8; 32], [2; 32], [3; 32]];
    let create = api::GroupCreate { group_id: group, epoch: 5, token_hash: yo_wire::sha256(&tokens[0]) };
    let created = client.post(format!("{}{}", hub.url(), api::GROUPS)).body(yo_wire::encode(&create)).send().unwrap();
    assert_eq!(created.status().as_u16(), 201);
    let commits: Vec<api::Commit> = (0..2)
        .map(|i| api::Commit {
            epoch: 5 + i as u64,
            token: tokens[i],
            next_token_hash: yo_wire::sha256(&tokens[i + 1]),
            envelope: yo_wire::envelope::seal(&[i as u8; 10]).unwrap().into(),
        })
        .collect();
    let post = |commit: &api::Commit| {
        let url = format!("{}{}", hub.url(), api::commit_path(&group));
        client.post(url).body(yo_wire::encode(commit)).send().unwrap().status().as_u16()
    };
    let fetched = |epoch: u64| {
        let url = format!("{}{}", hub.url(), api::commit_fetch_path(&group, epoch));
        client.get(url).send().unwrap().status().as_u16()
    };
    assert_eq!(commits.iter().map(post).collect::<Vec<_>>(), [200, 200]);
    assert_eq!([0, 4, 5, 6, 7].map(fetched), [404, 404, 200, 200, 404]);
    // 31 days on: the envelopes are gone, their acceptance is not.
    hub.hub.prune(yo_hub::now_ms() + 31 * 24 * 3600 * 1000).unwrap();
    assert_eq!([0, 4, 5, 6, 7].map(fetched), [404, 404, 410, 410, 404]);
    assert_eq!(commits.iter().map(post).collect::<Vec<_>>(), [200, 200], "the lost answers, repeated");
    let other = api::Commit { next_token_hash: [9; 32], ..commits[0].clone() };
    assert_eq!(post(&other), 409, "another commit on an epoch long gone");
}

/// Waits up to five seconds for the hub to have nothing under way at its inbox.
fn nothing_under_way(hub: &TestHub) -> bool {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while hub.hub.under_way() != (0, 0) {
        if std::time::Instant::now() > until {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    true
}

#[test]
fn a_long_poll_cut_off_leaves_nothing_behind() {
    let hub = TestHub::start();
    let http = Http::new(hub.url());
    let mut a = register(http.clone(), &username(random_u64() % 1_000_000_000), None);
    // A long poll, signed by A, sent as a client that gives up after a moment.
    let path = api::inbox_path(60);
    http.fault(&path, Fault::Answer(200));
    a.signed_for_tests("GET", &path, &[]).unwrap();
    let (_, _, headers, _) = http.recorded.lock().unwrap().pop().unwrap();
    let mut request = reqwest::blocking::Client::new()
        .get(format!("{}{path}", hub.url()))
        .timeout(std::time::Duration::from_millis(300));
    for (name, value) in headers {
        request = request.header(name, value);
    }
    assert!(request.send().unwrap_err().is_timeout());
    assert!(nothing_under_way(&hub), "{:?}", hub.hub.under_way());
}

#[test]
fn private_keys_of_dead_key_packages_are_deleted() {
    let hub = TestHub::start();
    let mut a = register(Http::new(hub.url()), &username(random_u64() % 1_000_000_000), None);
    let made = a.own_key_packages();
    assert_eq!(made, yo_wire::limits::KEY_PACKAGE_TARGET as usize + 1);
    let start = yo_hub::now_ms();
    // A month: the last resort is replaced. Another month: the replaced one can no longer be in a Welcome.
    for months in 1..=2u64 {
        let at = start + months * 32 * 24 * 3600 * 1000;
        a.set_clock(Arc::new(move || at));
        a.maintenance().unwrap();
    }
    assert_eq!(a.own_key_packages(), made + 1, "two replacements made, the first one forgotten");
    // A year and more: the unused single-use ones expired too.
    let at = start + 400 * 24 * 3600 * 1000;
    a.set_clock(Arc::new(move || at));
    a.maintenance().unwrap();
    assert!(a.own_key_packages() <= 2, "{}", a.own_key_packages());
}

#[test]
fn a_repair_whose_answer_was_lost_is_settled_by_repeating_it() {
    let hub = TestHub::start();
    let (mut a, b, ha, _, pa, pb) = chat(&hub);
    let chat_id = a.chats()[0].1;
    // B moves to a new phone; A's repair commit is accepted, its answer lost.
    let mut b2 = new_phone(Http::new(hub.url()), &pb, &b);
    drop(b);
    ha.fault("/commit", Fault::LoseAnswer);
    assert!(a.send_yo(&pb).is_ok() || a.pending() > 0);
    // The next call repeats the commit, the hub confirms it, the held Yo goes to the new phone.
    let mut events = a.sync(0).unwrap();
    events.extend(a.send_yo(&pb).unwrap());
    let at_b2 = b2.sync(0).unwrap();
    assert_eq!(yos(&at_b2), 2, "{at_b2:?}");
    assert_eq!(b2.chats()[0].1, chat_id);
    b2.send_yo(&pa).unwrap();
    assert_eq!(yos(&a.sync(0).unwrap()), 1);
    assert_eq!(a.dropped() + b2.dropped(), 0, "{:?} {:?}", a.drop_reasons(), b2.drop_reasons());
}

#[test]
fn key_packages_whose_upload_answer_was_lost_still_open_chats() {
    let hub = TestHub::start();
    let base = random_u64() % 1_000_000_000;
    let target = username(base);
    let hc = Http::new(hub.url());
    let mut c = register(hc.clone(), &target, None);
    // Senders use up C's KeyPackages.
    let senders = yo_wire::limits::KEY_PACKAGE_TARGET as u64 - 10;
    for i in 1..=senders {
        register(Http::new(hub.url()), &username(base + i), None).send_yo(&target).unwrap();
    }
    // C refills; the hub stores the upload, the answer is lost.
    hc.fault("/v1/keypackages", Fault::LoseAnswer);
    let events = c.sync(0).unwrap();
    assert_eq!(events.iter().filter(|e| matches!(e, Event::NewChat { .. })).count() as u64, senders);
    // Twenty more senders take the old and the uploaded packages: C keeps the private keys of all of them.
    for i in senders + 1..=senders + 20 {
        register(Http::new(hub.url()), &username(base + i), None).send_yo(&target).unwrap();
    }
    let events = c.sync(0).unwrap();
    assert_eq!(events.iter().filter(|e| matches!(e, Event::NewChat { .. })).count(), 20);
    assert_eq!(c.dropped(), 0, "{:?}", c.drop_reasons());
}
