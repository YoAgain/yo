//! Sealed sender (spec 6.1): what the hub sees of a chat is envelopes to inboxes, nothing that ties them together.

use yo_core::{Client, Event, Push};
use yo_tests::{random_u64, Http, TestHub};

fn texts(c: &mut Client) -> Vec<String> {
    (0..3)
        .flat_map(|_| c.sync(0).unwrap())
        .filter_map(|e| match e {
            Event::Message { content, .. } => yo_tests::tag_of(&content),
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

/// A first message each way (a request is signed, unsealed: spec 4.10), then both take in each other's profile key
/// and sender certificate (the work after a sync): from here on every delivery is sealed. How many envelopes each
/// inbox got by then, to look only at what came after.
fn contacts(a: &mut Client, ha: &Http, b: &mut Client, hb: &Http) -> (usize, usize) {
    a.send(b.user_id(), yo_tests::tagged("hi".into())).unwrap();
    assert_eq!(texts(b), ["hi"]);
    b.send(a.user_id(), yo_tests::tagged("hi".into())).unwrap();
    assert_eq!(texts(a), ["hi"]);
    for _ in 0..2 {
        a.after_sync().unwrap();
        assert!(texts(b).is_empty());
        b.after_sync().unwrap();
        assert!(texts(a).is_empty());
    }
    (delivered(ha, b.user_id()).len(), delivered(hb, a.user_id()).len())
}

/// Between contacts, both ways: no envelope carries anything of the sender (its user id, account key,
/// device key), and each opens only for the account it went to.
#[test]
fn the_hub_cannot_tell_which_envelopes_belong_to_one_chat() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let (ha, hb) = (Http::new(hub.url()), Http::new(hub.url()));
    let mut a = Client::register_username(ha.clone(), &format!("Ann{n}"), None, None, Push::none()).unwrap();
    let mut b = Client::register_username(hb.clone(), &format!("Ben{n}"), None, None, Push::none()).unwrap();
    let (skip_b, skip_a) = contacts(&mut a, &ha, &mut b, &hb);
    for i in 0..3 {
        a.send(b.user_id(), yo_tests::tagged(format!("a{i}"))).unwrap();
        assert_eq!(texts(&mut b), [format!("a{i}")]);
        b.send(a.user_id(), yo_tests::tagged(format!("b{i}"))).unwrap();
        assert_eq!(texts(&mut a), [format!("b{i}")]);
    }
    let (to_b, to_a) = (delivered(&ha, b.user_id()).split_off(skip_b), delivered(&hb, a.user_id()).split_off(skip_a));
    assert!(to_b.len() >= 3 && to_a.len() >= 3, "{} {}", to_b.len(), to_a.len());
    let account = |c: &Client| yo_tests::account_key(&c.account_secret()).to_vec();
    for (envelopes, sender) in [(&to_b, &a), (&to_a, &b)] {
        let marks = [yo_tests::user_id(sender.user_id()).to_vec(), account(sender)];
        for envelope in envelopes.iter() {
            for mark in &marks {
                assert!(!envelope.windows(mark.len()).any(|w| w == mark.as_slice()), "the sender is in an envelope");
            }
        }
    }
    for envelope in &to_b {
        assert_eq!(envelope[0], yo_wire::envelope::SEALED);
        assert!(yo_core::sealed::testing::open(&b.account_secret(), envelope).is_ok());
        assert!(yo_core::sealed::testing::open(&a.account_secret(), envelope).is_err());
    }
    for envelope in &to_a {
        assert!(yo_core::sealed::testing::open(&a.account_secret(), envelope).is_ok());
    }
    // The session's first message (its PQXDH, a Kyber ciphertext: 4 KiB) went with the request; once answered, every
    // text is the same 1 KiB bucket.
    assert!(to_b.iter().chain(&to_a).all(|e| e.len() == 1024), "{:?}", to_b.iter().map(Vec::len).collect::<Vec<_>>());
}

/// An envelope not sealed to the account (bare, or sealed to someone else) is dropped, and the chat goes on.
#[test]
fn an_envelope_not_sealed_to_the_account_is_dropped() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let (ha, hb) = (Http::new(hub.url()), Http::new(hub.url()));
    let mut a = Client::register_username(ha.clone(), &format!("Amy{n}"), None, None, Push::none()).unwrap();
    let mut b = Client::register_username(hb.clone(), &format!("Bob{n}"), None, None, Push::none()).unwrap();
    contacts(&mut a, &ha, &mut b, &hb);
    b.send(a.user_id(), yo_tests::tagged("1".into())).unwrap();
    assert_eq!(texts(&mut a), ["1"]);
    b.send(a.user_id(), yo_tests::tagged("2".into())).unwrap();
    let last = delivered(&hb, a.user_id()).pop().unwrap();
    let (payload, certificate) = yo_core::sealed::testing::open(&a.account_secret(), &last).unwrap();
    let stranger = yo_tests::account_key(&[9; 32]);
    let dropped = a.dropped();
    let key = a.access_key_for_tests();
    // Bare: the hub refuses it. Sealed to someone else: taken, and dropped by A.
    let bare = yo_wire::envelope::pad(&payload, &yo_wire::envelope::BUCKETS).unwrap();
    assert_eq!(yo_tests::deliver_raw_with(&hub.url(), a.user_id(), a.access_key_for_tests().as_deref(), bare), 400);
    assert_eq!(
        yo_tests::deliver_raw_with(
            &hub.url(),
            a.user_id(),
            key.as_deref(),
            yo_core::sealed::testing::seal(&b.account_secret(), &certificate, &stranger, &payload).unwrap()
        ),
        202
    );
    assert_eq!(texts(&mut a), ["2"], "the real one still comes");
    assert_eq!(a.dropped(), dropped + 1, "{:?}", a.drop_reasons());
    b.send(a.user_id(), yo_tests::tagged("3".into())).unwrap();
    assert_eq!(texts(&mut a), ["3"]);
}

const HOUR: u64 = 3600 * 1000;
const DAY: u64 = 24 * HOUR;

fn wall() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}

/// A clock `ms` off the real time (tests: days gone by, or a phone set wrong).
fn clock(ms: i64) -> yo_core::Clock {
    std::sync::Arc::new(move || wall().saturating_add_signed(ms))
}

fn pair(hub: &TestHub, tag: &str) -> (Client, Client, std::sync::Arc<Http>, std::sync::Arc<Http>) {
    let n = random_u64() % 100_000;
    let (ha, hb) = (Http::new(hub.url()), Http::new(hub.url()));
    let mut a = Client::register_username(ha.clone(), &format!("A{tag}{n}"), None, None, Push::none()).unwrap();
    let mut b = Client::register_username(hb.clone(), &format!("B{tag}{n}"), None, None, Push::none()).unwrap();
    a.send(b.user_id(), yo_tests::tagged("hi".into())).unwrap();
    assert_eq!(texts(&mut b), ["hi"]);
    b.send(a.user_id(), yo_tests::tagged("hello".into())).unwrap();
    assert_eq!(texts(&mut a), ["hello"]);
    (a, b, ha, hb)
}

fn account_key(c: &Client) -> [u8; 32] {
    yo_tests::account_key(&c.account_secret())
}

/// A's next text to B goes with `certificate` in place of its own: dropped by B (not shown), the chat goes on once A
/// has its own again.
fn refused_with(a: &mut Client, b: &mut Client, certificate: Vec<u8>) {
    let dropped = b.dropped();
    a.set_sender_certificate_for_tests(Some(certificate));
    a.send(b.user_id(), yo_tests::tagged("forged".into())).unwrap();
    assert!(texts(b).is_empty());
    assert_eq!(b.dropped(), dropped + 1, "{:?}", b.drop_reasons());
    a.set_sender_certificate_for_tests(None);
    a.maintenance().unwrap();
    a.send(b.user_id(), yo_tests::tagged("real".into())).unwrap();
    let got = texts(b);
    assert!(got.contains(&"real".to_owned()), "{got:?}");
}

/// A certificate that expired before the hub took the envelope: refused.
#[test]
fn an_expired_certificate_is_refused() {
    let hub = TestHub::start();
    let (mut a, mut b, _, _) = pair(&hub, "Exp");
    let old = yo_tests::sender_certificate(&yo_tests::user_id(a.user_id()), 1, &account_key(&a), wall() - 2 * DAY);
    refused_with(&mut a, &mut b, old);
}

/// A certificate from a server certificate of another trust root: refused; and the hub's answer from another root is
/// not taken as this device's own (it sends identified meanwhile).
#[test]
fn a_certificate_of_another_trust_root_is_refused() {
    let hub = TestHub::start();
    let (mut a, mut b, _, _) = pair(&hub, "Root");
    let (other, other_public) = yo_hub::certificate::new_root();
    let signer =
        yo_hub::certificate::Signer::from_bytes(&yo_hub::certificate::server_file(&other, 1).unwrap()).unwrap();
    let foreign = signer.issue(&yo_tests::user_id(a.user_id()), 1, &account_key(&a), wall()).unwrap();
    refused_with(&mut a, &mut b, foreign);
    // A device pinning another root takes no certificate of this hub's: none in date, its Yos go identified.
    a.set_trust_roots(vec![other_public]);
    a.set_sender_certificate_for_tests(None);
    a.maintenance().unwrap();
    assert!(a.sender_certificate_for_tests().is_none());
}

/// A certificate naming A's user and device with another identity than the one that sealed: refused.
#[test]
fn a_certificate_for_another_identity_is_refused() {
    let hub = TestHub::start();
    let (mut a, mut b, _, _) = pair(&hub, "Idn");
    let other = yo_tests::account_key(&[5; 32]);
    let wrong = yo_tests::sender_certificate(&yo_tests::user_id(a.user_id()), 1, &other, wall());
    refused_with(&mut a, &mut b, wrong);
    // And one naming another user (B's) with A's own identity: refused too.
    let wrong = yo_tests::sender_certificate(&yo_tests::user_id(b.user_id()), 1, &account_key(&a), wall());
    refused_with(&mut a, &mut b, wrong);
}

/// A certificate is fetched again before it expires (less than 2 h left), not after: every Yo stays sealed.
#[test]
fn the_certificate_is_refreshed_before_it_expires() {
    let hub = TestHub::start();
    let (mut a, mut b, ha, _) = pair(&hub, "Ref");
    let first = a.sender_certificate_for_tests().unwrap();
    a.set_clock(clock((20 * HOUR) as i64));
    a.maintenance().unwrap();
    assert_eq!(a.sender_certificate_for_tests().unwrap(), first, "20 h: still the first");
    a.set_clock(clock((22 * HOUR) as i64));
    a.maintenance().unwrap();
    assert_ne!(a.sender_certificate_for_tests().unwrap(), first, "22 h: a new one");
    a.send(b.user_id(), yo_tests::tagged("sealed".into())).unwrap();
    assert_eq!(texts(&mut b), ["sealed"]);
    assert_eq!(delivered(&ha, b.user_id()).last().unwrap()[0], yo_wire::envelope::SEALED, "the Yo after the refresh");
}

/// Offline for 31 days (its certificate long expired): what goes first (a receipt, a Yo) goes at once, identified;
/// the certificate is fetched after it, and the next goes sealed. What a contact sealed 20 days ago with a certificate good then still
/// opens (checked at the time the hub took it).
#[test]
fn back_after_a_month_offline() {
    let hub = TestHub::start();
    let shift = |c: &mut Client, ms: i64| c.set_clock(clock(ms));
    hub.hub.shift_clock(-31 * DAY as i64);
    let (mut a, mut b, ha, hb) = pair(&hub, "Away");
    for (c, ms) in [(&mut a, -31 * DAY as i64), (&mut b, -31 * DAY as i64)] {
        shift(c, ms);
        c.maintenance().unwrap();
    }
    // Eleven days on, A writes; its certificate is from then: the first goes identified, the next sealed.
    hub.hub.shift_clock(-20 * DAY as i64);
    shift(&mut a, -20 * DAY as i64);
    let before = delivered(&ha, b.user_id()).len();
    a.send(b.user_id(), yo_tests::tagged("one".into())).unwrap();
    a.send(b.user_id(), yo_tests::tagged("two".into())).unwrap();
    let firsts = |sent: &[Vec<u8>]| sent.iter().map(|e| e[0]).collect::<Vec<_>>();
    let to_b = firsts(&delivered(&ha, b.user_id())[before..]);
    assert_eq!((to_b[0], *to_b.last().unwrap()), (yo_wire::VERSION, yo_wire::envelope::SEALED), "{to_b:?}");
    // Today: B, offline since, receives both (the sealed one with a certificate that expired 19 days ago).
    hub.hub.shift_clock(0);
    shift(&mut b, 0);
    let before = delivered(&hb, a.user_id()).len();
    assert_eq!(texts(&mut b), ["one", "two"]);
    b.send(a.user_id(), yo_tests::tagged("back".into())).unwrap();
    b.send(a.user_id(), yo_tests::tagged("again".into())).unwrap();
    shift(&mut a, 0);
    assert_eq!(texts(&mut a), ["back", "again"]);
    let to_a = firsts(&delivered(&hb, a.user_id())[before..]);
    assert_eq!((to_a[0], *to_a.last().unwrap()), (yo_wire::VERSION, yo_wire::envelope::SEALED), "{to_a:?}");
    assert_eq!(a.dropped() + b.dropped(), 0, "{:?} {:?}", a.drop_reasons(), b.drop_reasons());
}

/// Phones whose clocks are 3 h off (one ahead, one behind): certificates count from when they came by the phone's
/// own clock, and are checked at the hub's time: everything goes sealed and opens.
#[test]
fn clocks_hours_off_still_seal() {
    let hub = TestHub::start();
    let (mut a, mut b, ha, hb) = pair(&hub, "Clk");
    a.set_clock(clock(3 * HOUR as i64));
    b.set_clock(clock(-3 * HOUR as i64));
    a.maintenance().unwrap();
    b.maintenance().unwrap();
    let (sent_a, sent_b) = (delivered(&ha, b.user_id()).len(), delivered(&hb, a.user_id()).len());
    a.send(b.user_id(), yo_tests::tagged("ahead".into())).unwrap();
    assert_eq!(texts(&mut b), ["ahead"]);
    b.send(a.user_id(), yo_tests::tagged("behind".into())).unwrap();
    assert_eq!(texts(&mut a), ["behind"]);
    assert!(delivered(&ha, b.user_id())[sent_a..].iter().all(|e| e[0] == yo_wire::envelope::SEALED));
    assert!(delivered(&hb, a.user_id())[sent_b..].iter().all(|e| e[0] == yo_wire::envelope::SEALED));
}

/// The hub answers the certificate request with an error: the Yo goes at once, identified (signed), and B shows it as
/// its contact's, not as a request; once the hub gives one again, Yos go sealed.
#[test]
fn without_a_certificate_a_yo_goes_identified() {
    let hub = TestHub::start();
    let (mut a, mut b, ha, _) = pair(&hub, "Idf");
    a.set_sender_certificate_for_tests(None);
    ha.fault(yo_wire::api::CERTIFICATE, yo_tests::Fault::Answer(500));
    a.send(b.user_id(), yo_tests::tagged("identified".into())).unwrap();
    let events: Vec<Event> = (0..3).flat_map(|_| b.sync(0).unwrap()).collect();
    assert!(events.iter().any(
        |e| matches!(e, Event::Message { content, .. } if yo_tests::tag_of(content).as_deref() == Some("identified"))
    ));
    assert!(!events.iter().any(|e| matches!(e, Event::NewChat { .. })), "a contact's, no request");
    assert!(b.contacts().iter().any(|c| c.user == a.user_id() && c.accepted));
    let to_b = delivered(&ha, b.user_id());
    assert_eq!(to_b.last().unwrap()[0], yo_wire::VERSION);
    assert!(a.sender_certificate_for_tests().is_none(), "the hub refused it");
    // A minute on, the hub gives one: the next Yo after it goes sealed.
    a.set_clock(clock(61_000));
    a.maintenance().unwrap();
    a.send(b.user_id(), yo_tests::tagged("sealed".into())).unwrap();
    assert_eq!(texts(&mut b), ["sealed"]);
    assert_eq!(delivered(&ha, b.user_id()).last().unwrap()[0], yo_wire::envelope::SEALED);
}

/// A clock set back after the certificate came does not stretch its day: noticed, the next Yo goes identified, a new
/// certificate is fetched, the one after goes sealed again.
#[test]
fn a_clock_set_back_is_noticed() {
    let hub = TestHub::start();
    let (mut a, mut b, ha, _) = pair(&hub, "Back");
    a.set_clock(clock((12 * HOUR) as i64));
    a.send(b.user_id(), yo_tests::tagged("noon".into())).unwrap();
    a.set_clock(clock((6 * HOUR) as i64));
    let before = delivered(&ha, b.user_id()).len();
    a.send(b.user_id(), yo_tests::tagged("morning".into())).unwrap();
    a.send(b.user_id(), yo_tests::tagged("later".into())).unwrap();
    assert_eq!(texts(&mut b), ["noon", "morning", "later"]);
    let firsts: Vec<u8> = delivered(&ha, b.user_id())[before..].iter().map(|e| e[0]).collect();
    assert_eq!(firsts, [yo_wire::VERSION, yo_wire::envelope::SEALED]);
}
