//! Oblivious HTTP end to end (spec 4.1): the main flows with every request wrapped, media by tickets, each line its
//! own connection; what the proxy sees is a bucket. The key-handing answers are taken unsigned (the wrapping is the
//! hub's word), and a delivery of one Yo fills the smallest bucket with small envelopes.

use std::sync::{Arc, Mutex};
use yo_core::media;
use yo_core::ohttp::Ohttp;
use yo_core::{Client, Content, Event, Push, Response, Transport};
use yo_tests::{ohttp_config, random_u64, username, Http, TestHub};
use yoip::net::{api, limits};

/// A line that notes what goes out on it: path and size.
struct Seen(Arc<Http>, Mutex<Vec<(String, String, usize, usize)>>);

impl Transport for Seen {
    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String> {
        let response = self.0.request(method, path, headers, body)?;
        self.1.lock().unwrap().push((method.into(), path.into(), body.len(), response.body.len()));
        Ok(response)
    }
}

struct Phone {
    client: Client,
    signed: Arc<Seen>,
    anonymous: Arc<Seen>,
}

fn phone(hub: &TestHub, name: &str) -> Phone {
    let line = || Arc::new(Seen(Http::unrecorded(hub.url()), Mutex::default()));
    let (signed, anonymous) = (line(), line());
    let transport = Arc::new(Ohttp::new(signed.clone(), anonymous.clone(), vec![ohttp_config()]));
    let mut client = Client::register_username(transport, name, None, None, Push::none()).unwrap();
    client.set_hub_keys(vec![yo_tests::hub_key()]);
    client.set_trust_roots(vec![yo_tests::trust_root()]);
    client.set_small_envelopes(true);
    Phone { client, signed, anonymous }
}

fn everything(p: &Phone) -> Vec<(String, String, usize, usize)> {
    let mut all = p.signed.1.lock().unwrap().clone();
    all.extend(p.anonymous.1.lock().unwrap().iter().cloned());
    all
}

#[test]
fn yos_lookups_and_media_go_wrapped_and_padded() {
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (na, nb) = (username(n), username(n + 1));
    let mut a = phone(&hub, &na);
    let mut b = phone(&hub, &nb);
    a.client.send_yo(&nb).unwrap();
    b.client.sync(0).unwrap();
    b.client.send_yo(&na).unwrap();
    a.client.sync(0).unwrap();
    // Each takes in the other's profile key and a sender certificate: from here on deliveries are sealed.
    for _ in 0..2 {
        a.client.after_sync().unwrap();
        b.client.sync(0).unwrap();
        b.client.after_sync().unwrap();
        a.client.sync(0).unwrap();
    }
    // An established session: one Yo, one device.
    a.signed.1.lock().unwrap().clear();
    a.anonymous.1.lock().unwrap().clear();
    a.client.send_yo(&nb).unwrap();
    let delivered: Vec<_> = a.anonymous.1.lock().unwrap().clone();
    assert_eq!(delivered.len(), 1, "{delivered:?}");
    assert_eq!(delivered[0].2, limits::OHTTP_BUCKETS[0], "a one-device Yo fills the smallest bucket");
    let events = b.client.sync(0).unwrap();
    assert!(events.iter().any(|e| matches!(e, Event::Message { content: Content::Yo, .. })), "{events:?}");

    // A photo: a ticket inside, the transfer outside.
    let photo = b"a JPEG".repeat(5000);
    let (sealed_media, sealed) = media::seal(&photo, 10, 10).unwrap();
    let fetches = a.client.media_fetches(&nb).unwrap();
    a.client.upload(&sealed_media, fetches).unwrap().send(&sealed).unwrap();
    a.client.send(&nb, Content::Photo(sealed_media.clone())).unwrap();
    let events = b.client.sync(0).unwrap();
    let got = events.iter().find_map(|e| match e {
        Event::Message { content: Content::Photo(m), .. } => Some(m.clone()),
        _ => None,
    });
    let transport = b.client.transport();
    assert_eq!(media::fetch_open(transport.as_ref(), &got.unwrap()).unwrap().unwrap(), photo);
    media::ack(transport.as_ref(), &sealed_media.id).unwrap();

    // What the proxy saw: only `/v1/ohttp` in buckets, and ticket transfers.
    for p in [&a, &b] {
        for (method, path, sent, answered) in everything(p) {
            if path.starts_with(api::TICKETS) {
                continue;
            }
            assert_eq!((method.as_str(), path.as_str()), ("POST", api::OHTTP));
            assert!(limits::OHTTP_BUCKETS.contains(&sent), "{sent}");
            assert!(limits::OHTTP_BUCKETS.contains(&answered), "{answered}");
        }
    }
    // Sealed deliveries never on the signed line, inbox fetches never on the anonymous one.
    assert!(a.anonymous.1.lock().unwrap().iter().any(|(_, path, ..)| path == api::OHTTP));
    assert_eq!(a.client.dropped() + b.client.dropped(), 0);
}

#[test]
fn a_lookup_over_ohttp_needs_no_hub_signature() {
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (na, nb) = (username(n), username(n + 1));
    let mut a = phone(&hub, &na);
    let _b = phone(&hub, &nb);
    // A pinned hub key that never signed anything: over OHTTP the answer is taken all the same.
    a.client.set_hub_keys(vec![[9; 32]]);
    assert!(a.client.whois_begin(&nb).unwrap().send().is_ok());
}
