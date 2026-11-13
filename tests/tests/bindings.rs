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
    assert!(!yo::username_free(app(), name.to_lowercase()).unwrap(), "taken in any spelling");
    assert!(matches!(yo::username_free(app(), "x".into()), Err(yo::YoError::Refused { status: 400 })));
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

/// The app's transport, holding the long poll (`wait` > 0) until the test lets it go.
struct Held {
    app: App,
    waiting: Mutex<std::sync::mpsc::Sender<()>>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}

impl HttpTransport for Held {
    fn request(&self, method: String, path: String, headers: Vec<yo::Header>, body: Vec<u8>) -> HttpResult {
        if path.contains("wait=") && !path.ends_with("wait=0") {
            self.waiting.lock().unwrap().send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
        }
        self.app.request(method, path, headers, body)
    }
}

#[test]
fn the_app_layer_runs_other_calls_while_sync_waits() {
    use std::sync::mpsc::channel;
    use std::time::Duration;
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (phone(n), phone(n + 1));
    let (waiting, waited) = channel();
    let (release, released) = channel();
    let held =
        Arc::new(Held { app: App(Http::new(hub.url())), waiting: Mutex::new(waiting), release: Mutex::new(released) });
    yo::request_code(held.clone(), pa.clone()).unwrap();
    let a = YoClient::register(
        held,
        Arc::new(File::default()),
        vec![7; 32],
        pa.clone(),
        "000000".into(),
        Some("ALICE".into()),
        0,
        String::new(),
    )
    .unwrap();
    let b = client(&hub, &pb, "BOB", Arc::new(File::default()));
    let syncing = {
        let a = a.clone();
        std::thread::spawn(move || a.sync(25))
    };
    waited.recv_timeout(Duration::from_secs(30)).expect("the long poll starts");
    // While A's sync waits: A sends, stores its backup, reads its contacts; B writes to A.
    let (done, finished) = channel();
    {
        let (a, pb) = (a.clone(), pb.clone());
        std::thread::spawn(move || {
            a.send_yo(pb).unwrap();
            a.put_backup(yo::YoBackup {
                by_passkey: Vec::new(),
                by_code: vec![2; 60],
                sealed: vec![3; 200],
                locator: vec![4; 16],
            })
            .unwrap();
            done.send(a.contacts().len()).unwrap();
        });
    }
    assert_eq!(finished.recv_timeout(Duration::from_secs(30)).expect("calls do not wait for the sync"), 1);
    assert!(b.sync(0).unwrap().iter().any(|e| matches!(e, YoEvent::Message { kind: MessageKind::Yo, .. })));
    b.send_yo(pa).unwrap();
    // The answer, signed before the send, is processed against the state after it.
    release.send(()).unwrap();
    let events = syncing.join().unwrap().unwrap();
    assert!(events.iter().any(|e| matches!(e, YoEvent::Message { kind: MessageKind::Yo, .. })));
    assert_eq!(a.contacts().len(), 1);
    a.send_yo(b.user_id()).unwrap();
    assert!(b.sync(0).unwrap().iter().any(|e| matches!(e, YoEvent::Message { kind: MessageKind::Yo, .. })));
}

/// The app's transport, holding blob uploads until the test lets them go.
struct HeldUpload {
    app: App,
    waiting: Mutex<std::sync::mpsc::Sender<()>>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}

impl HttpTransport for HeldUpload {
    fn request(&self, method: String, path: String, headers: Vec<yo::Header>, body: Vec<u8>) -> HttpResult {
        if method == "PUT" && path.starts_with("/v1/blobs/") {
            self.waiting.lock().unwrap().send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
        }
        self.app.request(method, path, headers, body)
    }
}

#[test]
fn the_app_layer_sends_a_photo_to_a_new_friend_and_yos_go_while_it_uploads() {
    use std::sync::mpsc::channel;
    use std::time::Duration;
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (pa, pb, pc) = (phone(n), phone(n + 1), phone(n + 2));
    let (waiting, waited) = channel();
    let (release, released) = channel();
    let held = Arc::new(HeldUpload {
        app: App(Http::new(hub.url())),
        waiting: Mutex::new(waiting),
        release: Mutex::new(released),
    });
    yo::request_code(held.clone(), pa.clone()).unwrap();
    let a = YoClient::register(
        held,
        Arc::new(File::default()),
        vec![7; 32],
        pa.clone(),
        "000000".into(),
        Some("ALICE".into()),
        0,
        String::new(),
    )
    .unwrap();
    let b = client(&hub, &pb, "BOB", Arc::new(File::default()));
    let c = client(&hub, &pc, "CAROL", Arc::new(File::default()));
    // A photo to B, who has no chat with A yet: the upload opens it, and is held on its way.
    let photo = b"JPEG".repeat(10_000);
    let sealed = yo::seal_media(photo.clone(), 1600, 1200).unwrap();
    let uploading = {
        let (a, pb, media) = (a.clone(), pb.clone(), sealed.media.clone());
        std::thread::spawn(move || a.upload_media(pb, media, sealed.ciphertext))
    };
    waited.recv_timeout(Duration::from_secs(30)).expect("the upload starts");
    // Meanwhile Yos go: the upload does not hold the client.
    let (done, finished) = channel();
    {
        let (a, pc) = (a.clone(), pc.clone());
        std::thread::spawn(move || done.send(a.send_yo(pc).is_ok()).unwrap());
    }
    assert!(finished.recv_timeout(Duration::from_secs(30)).expect("a Yo does not wait for the upload"));
    assert!(c.sync(0).unwrap().iter().any(|e| matches!(e, YoEvent::Message { kind: MessageKind::Yo, .. })));
    release.send(()).unwrap();
    uploading.join().unwrap().unwrap();
    a.send_photo(pb.clone(), sealed.media).unwrap();
    // B: a new chat and a Yo with a photo, opened once.
    let events = b.sync(0).unwrap();
    let media = events
        .iter()
        .find_map(|e| match e {
            YoEvent::Message { kind: MessageKind::Photo { media }, .. } => Some(media.clone()),
            _ => None,
        })
        .expect("a photo");
    assert_eq!((media.width, media.height), (1600, 1200));
    assert_eq!(b.fetch_media(media.clone()).unwrap(), Some(photo));
    assert_eq!(b.fetch_media(media).unwrap(), None, "fetched by its one device: gone");
    assert!(yo::media_retry_later(&yo::YoError::Refused { status: 507 }));
    assert!(!yo::media_retry_later(&yo::YoError::NotOnYo));
}
