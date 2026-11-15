//! Spec 6.10: answers that hand out keys (a lookup, a delivery's `409`) are signed by the hub's key, which the app
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
        if path != yo_wire::api::LOOKUP || response.status != 200 {
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
    let stranger = ed25519_dalek::SigningKey::from_bytes(&[9; 32]).verifying_key().to_bytes();
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
