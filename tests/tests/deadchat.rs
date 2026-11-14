//! A chat lost to one side (spec 4.9): a device away longer than the hub keeps what waits for it (30 days) misses a
//! commit for good and cannot read its peer any more. It restarts the chat; the peer sends again what it missed.

use std::sync::Arc;
use yo_core::{Client, Content, Event, Push};
use yo_tests::{random_u64, Http, TestHub};

fn texts(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Message { content: Content::Text(t), .. } => Some(t.clone()),
            _ => None,
        })
        .collect()
}

fn drain(c: &mut Client) -> Vec<String> {
    (0..3).flat_map(|_| texts(&c.sync(0).unwrap())).collect()
}

fn day(n: u64) -> u64 {
    n * 24 * 3600 * 1000
}

#[test]
fn a_chat_lost_after_a_month_away_is_restarted_and_nothing_after_is_lost() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let mut a = Client::register_username(Http::new(hub.url()), &format!("Away{n}"), None, None, Push::none()).unwrap();
    let mut b = Client::register_username(Http::new(hub.url()), &format!("Here{n}"), None, None, Push::none()).unwrap();
    a.send(b.user_id(), Content::Text("1".into())).unwrap();
    assert_eq!(drain(&mut b), ["1"]);
    b.send(a.user_id(), Content::Text("2".into())).unwrap();
    assert_eq!(drain(&mut a), ["2"]);
    // A is away. A week on, B's chat updates (a commit) and B writes.
    let later = yo_hub::now_ms() + day(8);
    b.set_clock(Arc::new(move || later));
    b.send(a.user_id(), Content::Text("3".into())).unwrap();
    // After 30 days the hub forgets what waited for A, the commit among it.
    hub.hub.prune(yo_hub::now_ms() + day(31)).unwrap();
    b.send(a.user_id(), Content::Text("4".into())).unwrap();
    // A is back: B's "4" is from an epoch A cannot reach. A restarts the chat; B sends again what A missed.
    assert!(drain(&mut a).is_empty());
    let mut missed = texts(&a.send(b.user_id(), Content::Text("5".into())).unwrap());
    assert_eq!(drain(&mut b), ["5"]);
    missed.extend(drain(&mut a));
    assert_eq!(missed, ["3", "4"], "what A missed, once, in order");
    b.send(a.user_id(), Content::Text("6".into())).unwrap();
    // A send receives first: what comes with it counts.
    let mut got = texts(&a.send(b.user_id(), Content::Text("7".into())).unwrap());
    got.extend(drain(&mut a));
    assert_eq!(got, ["6"], "a dropped {:?}, a chats {:?}, b chats {:?}", a.drop_reasons(), a.chats(), b.chats());
    assert_eq!(drain(&mut b), ["7"]);
    assert_eq!((a.chats().len(), b.chats().len()), (1, 1), "one chat each, the new one");
    assert_eq!(a.chats()[0].1, b.chats()[0].1);
    assert_eq!((a.pending(), b.pending()), (0, 0));
    assert_eq!(b.dropped(), 0, "{:?}", b.drop_reasons());
}
