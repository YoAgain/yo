//! Spec 4.1: answers that hand out keys (a lookup, a delivery's `409`) are signed by the hub's key, which the app
//! ships; a client that pins it takes no keys the hub did not sign for its very question.

use std::sync::{Arc, Mutex};
use yo_core::{Client, Content, Error, Event, Push, Response, Transport};
use yo_tests::{hub_key, random_u64, Http, TestHub};

/// How a party between the client and the hub tampers with lookup answers.
#[derive(Clone, Copy, PartialEq)]
enum Tamper {
    None,
    /// The signature cut off.
    Unsigned,
    /// The first lookup answer played back for every later one.
    Replay,
}

struct Between {
    http: Arc<Http>,
    tamper: Tamper,
    first: Mutex<Option<Vec<u8>>>,
}

impl Transport for Between {
    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String> {
        let mut response = self.http.request(method, path, headers, body)?;
        if path != yoip::net::api::LOOKUP
            || response.status != 200
            || !headers.iter().any(|(name, _)| name == yoip::net::hubsig::NONCE_HEADER)
        {
            return Ok(response);
        }
        match self.tamper {
            Tamper::None => {}
            Tamper::Unsigned => response.body.truncate(response.body.len().saturating_sub(64)),
            Tamper::Replay => {
                let mut first = self.first.lock().unwrap();
                match &*first {
                    Some(old) => response.body = old.clone(),
                    None => *first = Some(response.body.clone()),
                }
            }
        }
        Ok(response)
    }
}

fn client(hub: &TestHub, tag: &str, tamper: Tamper, key: Option<[u8; 32]>) -> Client {
    let transport = Arc::new(Between { http: Http::new(hub.url()), tamper, first: Mutex::new(None) });
    let name = format!("{tag}{}", random_u64() % 1_000_000);
    let mut client = Client::register_username(transport, &name, None, None, Push::none()).unwrap();
    if let Some(key) = key {
        client.set_hub_keys(vec![key]);
    }
    client
}

fn yos(events: &[Event]) -> usize {
    events.iter().filter(|e| matches!(e, Event::Message { content: Content::Yo, .. })).count()
}

#[test]
fn a_pinned_client_takes_keys_the_hub_signed() {
    let hub = TestHub::start();
    let mut a = client(&hub, "SigA", Tamper::None, Some(hub_key()));
    let mut b = client(&hub, "SigB", Tamper::None, Some(hub_key()));
    a.send_yo(b.user_id()).unwrap();
    assert_eq!(yos(&b.sync(0).unwrap()), 1);
    b.send_yo(a.user_id()).unwrap();
    assert_eq!(yos(&a.sync(0).unwrap()), 1);
    assert!(a.security_status().hub_signed);
}

#[test]
fn keys_signed_by_another_key_or_not_at_all_are_not_taken() {
    let hub = TestHub::start();
    let stranger = yo_hub::config::hub_public_key(&[9; 32]);
    let b = client(&hub, "SigB", Tamper::None, None);
    for (tamper, key) in [(Tamper::None, stranger), (Tamper::Unsigned, hub_key())] {
        let mut a = client(&hub, "SigA", tamper, Some(key));
        assert!(matches!(a.send_yo(b.user_id()), Err(Error::Protocol(_))));
        assert!(a.contacts().is_empty(), "nothing taken in");
    }
    // Without a pinned key (an app from before) it asks for no signature, and takes the answer as it comes.
    let mut old = client(&hub, "SigOld", Tamper::None, None);
    old.send_yo(b.user_id()).unwrap();
    assert!(!old.security_status().hub_signed);
}

#[test]
fn an_old_answer_played_back_is_not_taken() {
    let hub = TestHub::start();
    let mut a = client(&hub, "SigA", Tamper::Replay, Some(hub_key()));
    let b = client(&hub, "SigB", Tamper::None, None);
    let c = client(&hub, "SigC", Tamper::None, None);
    a.send_yo(b.user_id()).unwrap();
    // The lookup of C is answered with the signed answer about B: signed, but for another question.
    assert!(matches!(a.send_yo(c.user_id()), Err(Error::Protocol(_))));
    assert_eq!(a.contacts().len(), 1);
}

/// A hub that names a second device first: in each lookup, and once as a delivery's `409`.
struct DuplicateNumber {
    http: Arc<Http>,
    account_key: Mutex<Option<[u8; 32]>>,
    conflict: std::sync::atomic::AtomicBool,
}

impl Transport for DuplicateNumber {
    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String> {
        use yoip::net::api;
        let Some(account_key) = *self.account_key.lock().unwrap() else {
            return self.http.request(method, path, headers, body);
        };
        let other = api::Device { key: [9; 32], id: 1 };
        let delivery = method != "GET" && path.starts_with(api::INBOX) && path != api::INBOX_ACK;
        if delivery && !self.conflict.swap(true, std::sync::atomic::Ordering::Relaxed) {
            // The first delivery is not taken: a `409` naming two devices.
            let devices = api::Devices { account_key, devices: vec![other, api::Device { key: [8; 32], id: 1 }] };
            return Ok(Response { status: 409, body: yoip::encode(&devices) });
        }
        let mut response = self.http.request(method, path, headers, body)?;
        if path == api::LOOKUP && response.status == 200 {
            let mut answer: api::LookupBot = yoip::decode(&response.body).map_err(|e| e.to_string())?;
            answer.named.found.devices.insert(0, other);
            response.body = yoip::encode(&answer);
        }
        Ok(response)
    }
}

/// Spec 3.2: a roster naming two keys at the same number is refused
/// before anything is pinned or changed: the session with the known device goes on.
#[test]
fn duplicate_device_numbers_change_nothing() {
    let hub = TestHub::start();
    let transport = Arc::new(DuplicateNumber {
        http: Http::new(hub.url()),
        account_key: Mutex::new(None),
        conflict: Default::default(),
    });
    let name = format!("Two{}", random_u64() % 1_000_000);
    let mut a = Client::register_username(transport.clone(), &name, None, None, Push::none()).unwrap();
    let other = format!("One{}", random_u64() % 1_000_000);
    let mut stranger = Client::register_username(transport.clone(), &other, None, None, Push::none()).unwrap();
    let mut b = client(&hub, "SigB", Tamper::None, None);
    a.send_yo(b.user_id()).unwrap();
    assert_eq!(yos(&b.sync(0).unwrap()), 1);
    let key = yo_core::state::unhex(&a.account_key_of(b.user_id()).unwrap()).unwrap();
    *transport.account_key.lock().unwrap() = Some(key.try_into().unwrap());
    // A lookup by name (a name no contact has, spec 3.1) is refused, nothing pinned.
    assert!(matches!(stranger.send_yo(b.username().unwrap()), Err(Error::Protocol(_))));
    assert!(stranger.contacts().is_empty(), "nothing taken in");
    // A's pinned contact goes without a lookup; the `409` leaves the Yo waiting.
    let _ = a.send_yo(b.username().unwrap());
    assert_eq!(a.waiting(), 1);
    a.maintenance().unwrap();
    assert_eq!(a.waiting(), 0);
    assert_eq!(yos(&b.sync(0).unwrap()), 1, "{:?}", a.drop_reasons());
}
