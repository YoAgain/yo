//! The layer the apps call (UniFFI): the same flow through `YoClient`, with HTTP and the store as callbacks.

use std::sync::{Arc, Mutex};
use yo::{HttpResult, HttpTransport, MessageKind, StateStore, YoClient, YoEvent};
use yo_tests::{phone, random_u64, Http, TestHub};

struct App(Arc<Http>);

impl HttpTransport for App {
    fn request(&self, method: String, path: String, headers: Vec<yo::Header>, body: Vec<u8>) -> HttpResult {
        let headers: Vec<(String, String)> = headers.into_iter().map(|h| (h.name, h.value)).collect();
        match yo_core::Transport::request(self.0.as_ref(), &method, &path, &headers, &body) {
            Ok(r) => HttpResult { status: r.status, body: r.body, error: None },
            Err(e) => HttpResult { status: 0, body: Vec::new(), error: Some(e) },
        }
    }
}

#[derive(Default)]
struct File(Mutex<Vec<u8>>);

impl StateStore for File {
    fn save(&self, blob: Vec<u8>) {
        *self.0.lock().unwrap() = blob;
    }
}

fn client(hub: &TestHub, number: &str, name: &str, file: Arc<File>) -> Arc<YoClient> {
    let app = Arc::new(App(Http::new(hub.url())));
    yo::request_code(app.clone(), number.into()).unwrap();
    YoClient::register(app, file, vec![7; 32], number.into(), "000000".into(), Some(name.into()), 0, String::new())
        .unwrap()
}

#[test]
fn the_app_layer_works_end_to_end() {
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (phone(n), phone(n + 1));
    let file_a = Arc::new(File::default());
    let a = client(&hub, &pa, "ALICE", file_a.clone());
    let b = client(&hub, &pb, "BOB", Arc::new(File::default()));
    a.send_yo(pb.clone()).unwrap();
    let events = b.sync(0).unwrap();
    assert!(events.iter().any(|e| matches!(e, YoEvent::Message { kind: MessageKind::Yo, .. })));
    assert!(events.iter().any(|e| matches!(e, YoEvent::Profile { name, .. } if name == "ALICE")));
    assert_eq!(b.contacts().len(), 1);
    // A knows B by number; B knows A by user id (A wrote first).
    assert_eq!(b.safety_number(a.user_id()).unwrap(), a.safety_number(pb.clone()).unwrap());
    // The app is killed and started again from what the store saved.
    let saved = file_a.0.lock().unwrap().clone();
    assert!(!saved.is_empty());
    drop(a);
    let app = Arc::new(App(Http::new(hub.url())));
    let a = YoClient::restore(app, Arc::new(File::default()), vec![7; 32], saved).unwrap();
    b.send_yo(pa.clone()).unwrap();
    let events = a.sync(0).unwrap();
    assert!(events.iter().any(|e| matches!(e, YoEvent::Message { kind: MessageKind::Yo, .. })));
    assert!(YoClient::restore(
        Arc::new(App(Http::new(hub.url()))),
        Arc::new(File::default()),
        vec![8; 32],
        vec![1, 2, 3]
    )
    .is_err());
}
