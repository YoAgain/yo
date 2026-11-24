//! Sessions against a forged sender claim (security audit 2026-11-23: C1): what a payload claims about its sender is
//! never taken, nor waited on, before its message opens.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use yo_core::{Client, Content, Event, Push};
use yo_tests::{random_u64, Http, TestHub};

fn by_name(http: Arc<Http>, name: &str) -> Client {
    Client::register_username(http, name, None, None, Push::none()).unwrap()
}

fn texts(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Message { content, .. } => yo_tests::tag_of(content),
            _ => None,
        })
        .collect()
}

fn text(t: &str) -> Content {
    yo_tests::tagged(t.into())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}

/// An identified envelope (spec 6.1: `Padded(Payload)`, first byte 1) posted by `from` (signed by its own device, as
/// any registered account can) to `to`, as a request: its payload claims whatever the poster likes.
fn post_identified(url: &str, from: &Client, to: &Client, payload: &yo_wire::envelope::Payload) -> u16 {
    let envelope = yo_wire::envelope::pad(&yo_wire::encode(payload), &yo_wire::envelope::BUCKETS).unwrap();
    let path = yo_wire::api::deliver_path(&yo_tests::user_id(to.user_id()));
    // The device check from public keys (a stranger learns them from a lookup or a 409).
    let to_state = to.saved().state;
    let hash = yo_wire::devices::hash(&yo_tests::account_key(&to.account_secret()), &[to_state.device_key]);
    let state = from.saved().state;
    let time = now_ms();
    let signature = yo_core::identity::sign_with(
        &state.auth_secret,
        &yo_wire::request::to_be_signed("POST", &path, time, &envelope),
    );
    reqwest::blocking::Client::new()
        .post(format!("{url}{path}"))
        .header(yo_wire::api::HEADER_DEVICES, yo_wire::b64(&hash))
        .header(yo_wire::request::HEADER_DEVICE, yo_wire::b64(&state.device_id))
        .header(yo_wire::request::HEADER_TIME, time.to_string())
        .header(yo_wire::request::HEADER_SIGNATURE, yo_wire::b64(&signature))
        .body(envelope)
        .send()
        .unwrap()
        .status()
        .as_u16()
}

/// C, a stranger, posts identified envelopes whose payload claims to be from B's contact A (A's public account key,
/// A's device number, a made-up device key). None of them opens, so none makes B ask the hub for A's roster (spec 4.6)
/// nor parks anything: A's real message comes at once, B's recovery lookups are all left, nothing waits.
#[test]
fn a_strangers_forged_sender_claim_holds_back_nothing() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let mut a = by_name(Http::new(hub.url()), &format!("Aa{n}"));
    let mut b = by_name(Http::new(hub.url()), &format!("Ab{n}"));
    let c = by_name(Http::new(hub.url()), &format!("Ac{n}"));
    a.send(b.user_id(), text("hi")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["hi"]);
    b.accept(a.user_id()).unwrap();
    for _ in 0..2 {
        a.after_sync().unwrap();
        b.sync(0).unwrap();
        b.after_sync().unwrap();
        a.sync(0).unwrap();
    }
    a.send(b.user_id(), text("before")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["before"]);

    // B's clock (what its day's budget counts by) starts an hour into the next UTC day, so the run stays in one day.
    const DAY: u64 = 24 * 3600 * 1000;
    let clock = Arc::new(AtomicU64::new((now_ms() / DAY + 1) * DAY + 3600 * 1000));
    let read = clock.clone();
    b.set_clock(Arc::new(move || read.load(Ordering::Relaxed)));
    // Only two lookups for the day: a forgery that spent them would show at once.
    b.set_recovery_lookups_per_day(2);

    let (a_state, b_state) = (a.saved().state, b.saved().state);
    let forged = |kind: u8, message: Vec<u8>| yo_wire::envelope::Payload {
        account_key: yo_tests::account_key(&a.account_secret()),
        device_key: [8; 32],
        device_id: a_state.device_number,
        sent_at: now_ms(),
        copies: vec![yo_wire::envelope::ForDevice { device_key: b_state.device_key, kind, message: message.into() }],
    };
    // Garbage, and a well-formed session message between A's and B's identities that no device of A's made (it does
    // not decrypt): twenty of each, a minute apart. The well-formed ones may spend B's lookups on a decryption error's
    // roster (4.6), never park anything.
    let identity = |key: [u8; 32]| {
        libsignal_protocol::IdentityKey::new(libsignal_protocol::PublicKey::from_djb_public_key_bytes(&key).unwrap())
    };
    let (a_identity, b_identity) =
        (identity(yo_tests::account_key(&a.account_secret())), identity(yo_tests::account_key(&b.account_secret())));
    let well_formed = |i: u8| {
        let ratchet = libsignal_protocol::KeyPair::generate(&mut rand::rng()).public_key;
        let message = libsignal_protocol::SignalMessage::new(
            4,
            &[i; 32],
            None,
            ratchet,
            i as u32,
            0,
            &[i; 48],
            &a_identity,
            &b_identity,
            &[],
        )
        .unwrap();
        message.serialized().to_vec()
    };
    for i in 0..20u8 {
        clock.fetch_add(61_000, Ordering::Relaxed);
        assert_eq!(post_identified(&hub.url(), &c, &b, &forged(0, vec![i; 64])), 202);
        assert_eq!(post_identified(&hub.url(), &c, &b, &forged(0, well_formed(i))), 202);
        assert!(texts(&b.sync(0).unwrap()).is_empty());
        assert_eq!(b.deferred(), 0, "nothing parked by a claim");
    }
    assert!(
        b.drop_reasons().iter().any(|r| r.starts_with("session:")),
        "the well-formed ones were tried as session messages: {:?}",
        b.drop_reasons()
    );
    a.send(b.user_id(), text("real")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["real"], "A's message at once: {:?}", b.drop_reasons());
    assert_eq!(b.deferred(), 0, "nothing parked");
    a.send(b.user_id(), text("again")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["again"]);
}
