//! Sealed sender (spec 6.1): what the hub sees of a chat is envelopes to inboxes, nothing that ties them together.

use ed25519_dalek::SigningKey;
use yo_core::{Client, Content, Event, Push};
use yo_tests::{random_u64, Http, TestHub};

fn texts(c: &mut Client) -> Vec<String> {
    (0..3)
        .flat_map(|_| c.sync(0).unwrap())
        .filter_map(|e| match e {
            Event::Message { content: Content::Text(t), .. } => Some(t),
            _ => None,
        })
        .collect()
}

/// The envelopes a client posted to a user's inbox.
fn delivered(http: &Http, to: &str) -> Vec<Vec<u8>> {
    let path = yo_wire::api::deliver_path(&yo_tests::user_id(to));
    let sent = http.sent.lock().unwrap();
    sent.iter().filter(|(m, p, _)| m == "POST" && *p == path).map(|(_, _, b)| b.clone()).collect()
}

/// Both ways of a chat, Welcome, messages and commits: no envelope carries the group id, and each opens only for the account it went to.
#[test]
fn the_hub_cannot_tell_which_envelopes_belong_to_one_chat() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let (ha, hb) = (Http::new(hub.url()), Http::new(hub.url()));
    let mut a = Client::register_username(ha.clone(), &format!("Ann{n}"), None, None, Push::none()).unwrap();
    let mut b = Client::register_username(hb.clone(), &format!("Ben{n}"), None, None, Push::none()).unwrap();
    for i in 0..3 {
        a.send(b.user_id(), Content::Text(format!("a{i}"))).unwrap();
        assert_eq!(texts(&mut b), [format!("a{i}")]);
        b.send(a.user_id(), Content::Text(format!("b{i}"))).unwrap();
        assert_eq!(texts(&mut a), [format!("b{i}")]);
    }
    let group = a.chats()[0].1;
    let (to_b, to_a) = (delivered(&ha, b.user_id()), delivered(&hb, a.user_id()));
    assert!(to_b.len() >= 4 && to_a.len() >= 3, "{} {}", to_b.len(), to_a.len());
    for envelope in to_b.iter().chain(&to_a) {
        assert!(!envelope.windows(group.len()).any(|w| w == group), "the group id is in an envelope");
    }
    for envelope in &to_b {
        assert!(yo_core::seal::open(&b.account_secret(), envelope).is_ok());
        assert!(yo_core::seal::open(&a.account_secret(), envelope).is_err());
    }
    for envelope in &to_a {
        assert!(yo_core::seal::open(&a.account_secret(), envelope).is_ok());
    }
    // The seal's 48 bytes keep a text in the smallest bucket; only the Welcome is larger.
    assert!(to_b[1..].iter().chain(&to_a).all(|e| e.len() == 1024));
}

/// An envelope not sealed to the account (a bare MLS message, as before, or one sealed to someone else) is dropped,
/// and the chat goes on.
#[test]
fn an_envelope_not_sealed_to_the_account_is_dropped() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let hb = Http::new(hub.url());
    let mut a = Client::register_username(Http::new(hub.url()), &format!("Amy{n}"), None, None, Push::none()).unwrap();
    let mut b = Client::register_username(hb.clone(), &format!("Bob{n}"), None, None, Push::none()).unwrap();
    b.send(a.user_id(), Content::Text("1".into())).unwrap();
    assert_eq!(texts(&mut a), ["1"]);
    b.send(a.user_id(), Content::Text("2".into())).unwrap();
    let last = delivered(&hb, a.user_id()).pop().unwrap();
    let mls = yo_core::seal::open(&a.account_secret(), &last).unwrap();
    let stranger = SigningKey::from_bytes(&[9; 32]).verifying_key().to_bytes();
    let dropped = a.dropped();
    // Bare, as before the seal: the hub refuses it. Sealed to someone else: taken, and dropped by A.
    assert_eq!(yo_tests::deliver_raw(&hub.url(), a.user_id(), yo_wire::envelope::seal(&mls).unwrap()), 400);
    assert_eq!(yo_tests::deliver_raw(&hub.url(), a.user_id(), yo_core::seal::to(&stranger, &mls).unwrap()), 202);
    assert_eq!(texts(&mut a), ["2"], "the real one still comes");
    assert_eq!(a.dropped(), dropped + 1, "{:?}", a.drop_reasons());
    b.send(a.user_id(), Content::Text("3".into())).unwrap();
    assert_eq!(texts(&mut a), ["3"]);
}
