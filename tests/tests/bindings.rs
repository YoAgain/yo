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
fn the_app_layer_registers_by_username_and_restores_from_the_backup() {
    let hub = TestHub::start();
    let app = || Arc::new(App(Http::new(hub.url())));
    let name = format!("JOSÉ{}", random_u64() % 100_000);
    let register = |secret: Option<Vec<u8>>| {
        YoClient::register_username(
            app(),
            Arc::new(File::default()),
            vec![7; 32],
            name.clone(),
            secret,
            None,
            0,
            "".into(),
        )
    };
    assert!(yo::username_free(app(), name.clone()).unwrap());
    let old = register(None).unwrap();
    assert!(!yo::username_free(app(), name.clone()).unwrap(), "taken now");
    assert!(matches!(yo::username_free(app(), name.to_lowercase()), Err(yo::YoError::Refused { status: 400 })));
    assert_eq!(old.username(), Some(name.clone()));
    let secret = old.account_secret();
    assert_eq!(secret.len(), 32);
    // What the app makes: K wrapped twice and the account sealed (here stand-in bytes), and the code's locator.
    let backup =
        || yo::YoBackup { by_passkey: vec![1; 60], by_code: vec![2; 60], sealed: vec![3; 200], locator: vec![4; 16] };
    old.put_backup(backup()).unwrap();
    // A new phone: the record by the locator, then the name registered again with the account key it holds.
    let found = yo::fetch_backup(app(), vec![4; 16]).unwrap().unwrap();
    assert_eq!(
        (found.by_passkey, found.by_code, found.sealed, found.locator),
        (vec![1; 60], vec![2; 60], vec![3; 200], vec![4; 16])
    );
    assert!(yo::fetch_backup(app(), vec![5; 16]).unwrap().is_none());
    assert!(matches!(register(None), Err(yo::YoError::Refused { status: 409 })));
    let new = register(Some(secret)).unwrap();
    assert_eq!(new.user_id(), old.user_id());
    assert!(matches!(old.sync(0), Err(yo::YoError::Replaced)));
    assert!(yo::fetch_backup(app(), hex(&new.user_id())).unwrap().is_some(), "by the user id too");
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len() / 2).map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap()).collect()
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
