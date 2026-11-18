//! The layer the apps call (UniFFI): the same flow through `YoClient`, with HTTP and the store as callbacks.

use std::sync::{Arc, Mutex};
use yo::{HttpResult, HttpTransport, MessageKind, StateStore, YoClient, YoEvent};
use yo_tests::{random_u64, username, Disk, Http, Process, TestHub};

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
    fn save(&self, blob: Vec<u8>) -> Result<(), yo::YoError> {
        *self.0.lock().unwrap() = blob;
        Ok(())
    }
}

fn client(hub: &TestHub, username: &str, name: &str, file: Arc<File>) -> Arc<YoClient> {
    let app = Arc::new(App(Http::new(hub.url())));
    YoClient::register_username(
        app,
        file,
        vec![7; 32],
        username.into(),
        None,
        None,
        Some(name.into()),
        0,
        String::new(),
    )
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
    // What the app makes: its JSON under K, K wrapped by the words and the passkey, their auth keys.
    let (k, prf, words) = (vec![1; 32], vec![2; 32], yo_tests::backup::recovery_key([3; 16]));
    let passkey = yo::YoDoor::Passkey { prf: prf.clone() };
    let by_words = || yo::YoDoor::RecoveryKey { key: words.clone() };
    old.put_backup(k.clone(), b"{\"account\":1}".to_vec(), words.clone(), yo::YoPasskeyDoor::New { prf: prf.clone() })
        .unwrap();
    // A new phone: the record by the words alone, or by the passkey's user handle and PRF output; either opens it.
    let user = hex(&old.user_id());
    let found = yo::open_backup(app(), None, by_words()).unwrap().unwrap();
    assert_eq!((found.k.clone(), found.sealed.clone()), (k.clone(), b"{\"account\":1}".to_vec()));
    assert!(found.record.sealed.len() == 1024 && found.record.auth_passkey.len() == 32, "padded, both keys");
    let by_passkey = yo::open_backup(app(), Some(user.clone()), passkey).unwrap().unwrap();
    assert_eq!((by_passkey.k, by_passkey.sealed), (k.clone(), found.sealed.clone()));
    // Another passkey or other words fetch nothing: the hub answers as if there were no record.
    let other = yo::YoDoor::Passkey { prf: vec![9; 32] };
    assert!(yo::open_backup(app(), Some(user.clone()), other).unwrap().is_none());
    let other = yo::YoDoor::RecoveryKey { key: yo_tests::backup::recovery_key([4; 16]) };
    assert!(yo::open_backup(app(), None, other).unwrap().is_none());
    assert!(yo::open_backup(app(), None, yo::YoDoor::Passkey { prf: prf.clone() }).is_err(), "a passkey needs the id");
    // The name registered again with the account key it holds.
    assert!(matches!(register(None), Err(yo::YoError::Refused { status: 409 })));
    let new = register(Some(secret)).unwrap();
    assert_eq!(new.user_id(), old.user_id());
    assert!(matches!(old.sync(0), Err(yo::YoError::Replaced)));
    // New words, the passkey kept: the old words open nothing, the passkey still does.
    let renewed = yo_tests::backup::recovery_key([5; 16]);
    let keep = yo::YoPasskeyDoor::Keep { record: found.record };
    new.put_backup(k.clone(), b"{}".to_vec(), renewed.clone(), keep).unwrap();
    assert!(yo::open_backup(app(), None, by_words()).unwrap().is_none());
    let found = yo::open_backup(app(), None, yo::YoDoor::RecoveryKey { key: renewed }).unwrap().unwrap();
    let passkey = yo::YoDoor::Passkey { prf };
    assert_eq!(yo::open_backup(app(), Some(user), passkey).unwrap().unwrap().sealed, b"{}");
    assert_eq!(found.k, k);
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len() / 2).map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap()).collect()
}

#[test]
fn the_app_layer_works_end_to_end() {
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (username(n), username(n + 1));
    let file_a = Arc::new(File::default());
    let a = client(&hub, &pa, "ALICE", file_a.clone());
    let b = client(&hub, &pb, "BOB", Arc::new(File::default()));
    a.send_yo(pb.clone()).unwrap();
    let events = b.sync(0).unwrap();
    assert!(events.iter().any(|e| matches!(e, YoEvent::Message { kind: MessageKind::Yo, .. })));
    assert!(events.iter().any(|e| matches!(e, YoEvent::Profile { name, .. } if name == "ALICE")));
    assert_eq!(b.contacts().len(), 1);
    // A knows B by username; B knows A by user id (A wrote first).
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
    let (pa, pb) = (username(n), username(n + 1));
    let (waiting, waited) = channel();
    let (release, released) = channel();
    let held =
        Arc::new(Held { app: App(Http::new(hub.url())), waiting: Mutex::new(waiting), release: Mutex::new(released) });
    let a = YoClient::register_username(
        held,
        Arc::new(File::default()),
        vec![7; 32],
        pa.clone(),
        None,
        None,
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
            a.put_backup(vec![1; 32], b"{}".to_vec(), yo_tests::backup::recovery_key([2; 16]), yo::YoPasskeyDoor::None)
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
    let (pa, pb, pc) = (username(n), username(n + 1), username(n + 2));
    let (waiting, waited) = channel();
    let (release, released) = channel();
    let held = Arc::new(HeldUpload {
        app: App(Http::new(hub.url())),
        waiting: Mutex::new(waiting),
        release: Mutex::new(released),
    });
    let a = YoClient::register_username(
        held,
        Arc::new(File::default()),
        vec![7; 32],
        pa.clone(),
        None,
        None,
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
    let chat = a.media_chat(pb.clone()).unwrap();
    assert_eq!(chat.fetches, 1);
    let uploading = {
        let (a, media) = (a.clone(), sealed.media.clone());
        std::thread::spawn(move || a.upload_media(media, sealed.ciphertext, chat.fetches))
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
    b.ack_media(media.id.clone()).unwrap();
    assert_eq!(b.fetch_media(media).unwrap(), None, "fetched by its one device: gone");
    assert!(yo::retry_later(&yo::YoError::Refused { status: 507 }));
    assert!(!yo::retry_later(&yo::YoError::NotOnYo));
    // One rule for every path (spec 6.6): the core's, a too old app (426) and a full disk included.
    for status in [426, 429, 500, 503, 507] {
        assert!(yo::retry_later(&yo::YoError::Refused { status }), "{status}");
        assert!(yo_core::retryable(&yo_core::Error::Refused(status)), "{status}");
    }
    for status in [400, 404, 409, 413] {
        assert!(!yo::retry_later(&yo::YoError::Refused { status }), "{status}");
    }
}

#[test]
fn the_app_layer_acknowledges_each_batch_itself_and_sends_a_photo_once() {
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (username(n), username(n + 1));
    let a = client(&hub, &pa, "ALICE", Arc::new(File::default()));
    let file_b = Arc::new(File::default());
    let b = client(&hub, &pb, "BOB", file_b.clone());
    b.acknowledge_explicitly();
    a.send_yo(pb.clone()).unwrap();
    let first = b.sync(0).unwrap();
    assert_eq!(yos(&first, &a.user_id()), 1);
    // Another call ends before the app stored the first batch: it does not acknowledge it. The app stores the second.
    a.send_yo(pb.clone()).unwrap();
    let second = b.sync(0).unwrap();
    assert_eq!(yos(&second, &a.user_id()), 1);
    b.acknowledge(second);
    // A crash: the first batch comes again, as it was; the second does not.
    let restart = || {
        let saved = file_b.0.lock().unwrap().clone();
        let app = Arc::new(App(Http::new(hub.url())));
        let b = YoClient::restore(app, file_b.clone(), vec![7; 32], saved).unwrap();
        b.acknowledge_explicitly();
        b
    };
    let b = restart();
    let again = b.sync(0).unwrap();
    assert!(again == first, "the batch not acknowledged, again");
    b.acknowledge(again);
    let b = restart();
    assert!(b.sync(0).unwrap().is_empty(), "both acknowledged now");
    // A photo whose Yo the app sends again (its answer lost before it took it off its queue): it goes once.
    let sealed = yo::seal_media(b"JPEG".repeat(1000), 800, 600).unwrap();
    let chat = a.media_chat(pb.clone()).unwrap();
    a.upload_media(sealed.media.clone(), sealed.ciphertext, chat.fetches).unwrap();
    a.send_photo(pb.clone(), sealed.media.clone()).unwrap();
    a.send_photo(pb.clone(), sealed.media).unwrap();
    a.send_yo(pb.clone()).unwrap();
    let events = drain(&b);
    let photos =
        events.iter().filter(|e| matches!(e, YoEvent::Message { kind: MessageKind::Photo { .. }, .. })).count();
    assert_eq!((photos, yos(&events, &a.user_id())), (1, 1), "the photo once, the Yo after it still");
}

/// The app's transport, holding deliveries to one user until the test lets each go.
struct Gate {
    app: App,
    closed: Arc<std::sync::atomic::AtomicBool>,
    path: String,
    held: Mutex<std::sync::mpsc::Sender<()>>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}

impl HttpTransport for Gate {
    fn request(&self, method: String, path: String, headers: Vec<yo::Header>, body: Vec<u8>) -> HttpResult {
        if method == "POST" && path == self.path && self.closed.load(std::sync::atomic::Ordering::Relaxed) {
            self.held.lock().unwrap().send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
        }
        self.app.request(method, path, headers, body)
    }
}

fn yos(events: &[YoEvent], from: &str) -> usize {
    events.iter().filter(|e| matches!(e, YoEvent::Message { user, kind: MessageKind::Yo, .. } if user == from)).count()
}

/// Everything waiting for a client, until its inbox is empty.
fn drain(client: &YoClient) -> Vec<YoEvent> {
    let mut events = Vec::new();
    loop {
        let more = client.sync(0).unwrap();
        if more.is_empty() {
            return events;
        }
        events.extend(more);
    }
}

/// An answer by the way that holds no client (`send_begin`, the inbox just read) carries the name not yet told, as
/// `send` does: it went only with the next `maintenance` before.
#[test]
fn the_app_layer_tells_its_name_with_the_first_answer() {
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (username(n), username(n + 1));
    let a = client(&hub, &pa, "ALICE", Arc::new(File::default()));
    let b = client(&hub, &pb, "BOB", Arc::new(File::default()));
    a.send_yo(pb).unwrap();
    assert_eq!(yos(&drain(&b), &a.user_id()), 1);
    b.send_yo(a.user_id()).unwrap();
    let events = drain(&a);
    assert_eq!(yos(&events, &b.user_id()), 1);
    assert!(events.contains(&YoEvent::Profile { user: b.user_id(), name: "BOB".into() }), "the name with the answer");
}

#[test]
fn the_app_layer_sends_to_one_friend_while_a_delivery_to_another_waits() {
    use std::sync::mpsc::channel;
    use std::time::Duration;
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (pa, pb, pc) = (username(n), username(n + 1), username(n + 2));
    let b = client(&hub, &pb, "BOB", Arc::new(File::default()));
    let c = client(&hub, &pc, "CAROL", Arc::new(File::default()));
    let (held, holding) = channel();
    let (release, released) = channel();
    let b_user: [u8; 16] = hex(&b.user_id()).try_into().unwrap();
    let gate_closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let gate = Arc::new(Gate {
        app: App(Http::new(hub.url())),
        closed: gate_closed.clone(),
        path: yo_wire::api::deliver_path(&b_user),
        held: Mutex::new(held),
        release: Mutex::new(released),
    });
    let a = YoClient::register_username(
        gate,
        Arc::new(File::default()),
        vec![7; 32],
        pa.clone(),
        None,
        None,
        Some("ALICE".into()),
        0,
        String::new(),
    )
    .unwrap();
    // The chats, in use; then the door to B closes.
    a.send_yo(pc.clone()).unwrap();
    a.send_yo(pb.clone()).unwrap();
    gate_closed.store(true, std::sync::atomic::Ordering::Relaxed);
    // A Yo to B waits at the hub's door; meanwhile A sends to C, queues a second Yo to B, reads its contacts.
    let waiting = std::thread::spawn({
        let (a, pb) = (a.clone(), pb.clone());
        move || a.send_yo(pb)
    });
    holding.recv_timeout(Duration::from_secs(30)).expect("the delivery to B starts");
    let (done, finished) = channel();
    std::thread::spawn({
        let (a, pb, pc) = (a.clone(), pb.clone(), pc.clone());
        move || {
            a.send_yo(pc).unwrap();
            a.send_yo(pb).unwrap(); // behind the one on its way, not past it
            done.send(a.contacts().len()).unwrap();
        }
    });
    assert_eq!(finished.recv_timeout(Duration::from_secs(30)).expect("calls do not wait for the delivery"), 2);
    assert_eq!(yos(&drain(&c), &a.user_id()), 2);
    // Let go: the Yo on its way, then the one queued behind it.
    release.send(()).unwrap();
    holding.recv_timeout(Duration::from_secs(30)).expect("the queued Yo follows");
    release.send(()).unwrap();
    waiting.join().unwrap().unwrap();
    assert_eq!(yos(&drain(&b), &a.user_id()), 3);
}

/// A name that changed hands: the app's Yo to it goes to whoever has it now (asked of the hub, spec 4.2), not to the
/// contact that had it.
#[test]
fn the_app_layer_sends_a_name_to_whoever_has_it_now() {
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (username(n), username(n + 1));
    let a = client(&hub, &pa, "ALICE", Arc::new(File::default()));
    let b = client(&hub, &pb, "BOB", Arc::new(File::default()));
    b.send_yo(pa.clone()).unwrap();
    assert_eq!(yos(&drain(&a), &b.user_id()), 1);
    a.set_username(Some(username(n + 2))).unwrap();
    let c = client(&hub, &pa, "CAROL", Arc::new(File::default()));
    b.send_yo(pa.clone()).unwrap();
    assert_eq!(yos(&drain(&c), &b.user_id()), 1);
    assert_eq!(yos(&drain(&a), &b.user_id()), 0);
    assert!(b.contacts().iter().all(|k| k.user == c.user_id() || k.handles.is_empty()));
}

/// The app's transport over a phone's network: every request takes a round trip.
struct Slow(App);

impl HttpTransport for Slow {
    fn request(&self, method: String, path: String, headers: Vec<yo::Header>, body: Vec<u8>) -> HttpResult {
        std::thread::sleep(std::time::Duration::from_millis(40));
        self.0.request(method, path, headers, body)
    }
}

/// Like the app under a thumb, over a phone's network: A sends bursts to 20 friends at once, each from its own
/// thread, while those friends send bursts to A and A's inbox waits. Every Yo arrives once, both ways, in bounded time
/// (sends to several friends and the inbox share the network rather than taking turns); the chats still work.
#[test]
fn the_app_layer_sends_and_receives_bursts_with_many_friends_at_once() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};
    const FRIENDS: u64 = 20;
    const BURST: usize = 5;
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let pa = username(n);
    let slow = Arc::new(Slow(App(Http::new(hub.url()))));
    let a = YoClient::register_username(
        slow,
        Arc::new(File::default()),
        vec![7; 32],
        pa.clone(),
        None,
        None,
        None,
        0,
        "".into(),
    )
    .unwrap();
    let friends: Vec<(String, Arc<YoClient>)> = (1..=FRIENDS)
        .map(|i| {
            let p = username(n + i);
            let friend = client(&hub, &p, "FRIEND", Arc::new(File::default()));
            (p, friend)
        })
        .collect();
    // The chats exist: A's first Yo to each, taken in.
    for (p, friend) in &friends {
        a.send_yo(p.clone()).unwrap();
        assert_eq!(yos(&drain(friend), &a.user_id()), 1);
    }
    let started = Instant::now();
    let stop = Arc::new(AtomicBool::new(false));
    let got = Arc::new(Mutex::new(Vec::<YoEvent>::new()));
    // A's inbox, waiting as the app's does (a short wait, so the test ends soon).
    let listening = std::thread::spawn({
        let (a, got, stop) = (a.clone(), got.clone(), stop.clone());
        move || {
            while !stop.load(Ordering::Relaxed) {
                let events = a.sync(1).unwrap();
                got.lock().unwrap().extend(events);
            }
        }
    });
    let (finished, done) = std::sync::mpsc::channel();
    // What each friend's sends brought (a send may receive first).
    let heard: Vec<Arc<Mutex<Vec<YoEvent>>>> = friends.iter().map(|_| Arc::default()).collect();
    let mut threads = Vec::new();
    for ((p, friend), heard) in friends.iter().zip(&heard) {
        threads.push(std::thread::spawn({
            let (a, p, got, finished) = (a.clone(), p.clone(), got.clone(), finished.clone());
            move || {
                for _ in 0..BURST {
                    let events = a.send_yo(p.clone()).unwrap();
                    got.lock().unwrap().extend(events);
                }
                finished.send(()).unwrap();
            }
        }));
        threads.push(std::thread::spawn({
            let (friend, pa, finished, heard) = (friend.clone(), pa.clone(), finished.clone(), heard.clone());
            move || {
                for _ in 0..BURST {
                    let events = friend.send_yo(pa.clone()).unwrap();
                    heard.lock().unwrap().extend(events);
                }
                finished.send(()).unwrap();
            }
        }));
    }
    // Bounded: a send that hangs fails the test instead of holding it.
    for _ in 0..threads.len() {
        let left = Duration::from_secs(60).saturating_sub(started.elapsed());
        done.recv_timeout(left).expect("the sends end in time");
    }
    for thread in threads {
        thread.join().unwrap();
    }
    let everyone: Vec<String> = friends.iter().map(|(_, f)| f.user_id()).collect();
    while everyone.iter().any(|user| yos(&got.lock().unwrap(), user) < BURST) {
        assert!(started.elapsed() < Duration::from_secs(60), "Yos to A are slow to arrive");
        std::thread::sleep(Duration::from_millis(50));
    }
    stop.store(true, Ordering::Relaxed);
    listening.join().unwrap();
    for user in &everyone {
        assert_eq!(yos(&got.lock().unwrap(), user), BURST, "each Yo to A once");
    }
    for ((_, friend), heard) in friends.iter().zip(&heard) {
        let mut events = std::mem::take(&mut *heard.lock().unwrap());
        events.extend(drain(friend));
        assert_eq!(yos(&events, &a.user_id()), BURST, "each Yo from A once");
    }
    assert!(started.elapsed() < Duration::from_secs(60));
    assert_eq!(a.contacts().len(), FRIENDS as usize);
    // The chats are in order both ways.
    for (p, friend) in &friends {
        a.send_yo(p.clone()).unwrap();
        assert_eq!(yos(&drain(friend), &a.user_id()), 1);
        friend.send_yo(pa.clone()).unwrap();
        assert_eq!(yos(&drain(&a), &friend.user_id()), 1);
    }
}

#[test]
fn the_app_layer_asks_who_a_typed_name_is_before_the_first_yo() {
    let hub = TestHub::start();
    let app = || Arc::new(App(Http::new(hub.url())));
    let named = |name: &str| {
        YoClient::register_username(
            app(),
            Arc::new(File::default()),
            vec![7; 32],
            name.into(),
            None,
            None,
            None,
            0,
            "".into(),
        )
        .unwrap()
    };
    let n = random_u64() % 100_000;
    let (bot, me) = (named(&format!("YoBot{n}")), named(&format!("Ann{n}")));
    let found = me.whois(format!("yobot{n}")).unwrap().expect("on Yo");
    assert_eq!((found.user, found.username), (bot.user_id(), Some(format!("YoBot{n}"))));
    assert!(me.whois(format!("nobody{n}")).unwrap().is_none());
    assert!(me.whois(format!("ann{n}")).is_err(), "oneself");
    assert!(me.contacts().is_empty(), "asking makes no contact");
    // The first Yo by the user id it gave: the contact has the owner's spelling.
    me.send_yo(bot.user_id()).unwrap();
    assert_eq!(me.contacts()[0].handles, vec![format!("YoBot{n}")]);
    assert_eq!(yos(&drain(&bot), &me.user_id()), 1);
}

/// The hub takes so many deliveries a minute from one sender (spec 6.6); a burst over it waits in the outbox and goes
/// out, in order, as the app sends it again. Nothing is lost.
#[test]
fn the_app_layer_sends_a_burst_over_the_hubs_limit_later_in_order() {
    use std::time::{Duration, Instant};
    const BURST: usize = 60; // twice the limit: over it also on a slow machine, whose burst takes a while
    let mut config = yo_hub::config::Config::for_tests();
    config.limits.deliver_per_ip_minute = 30;
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (username(n), username(n + 1));
    let a = client(&hub, &pa, "ALICE", Arc::new(File::default()));
    let b = client(&hub, &pb, "BOB", Arc::new(File::default()));
    a.send_yo(pb.clone()).unwrap();
    assert_eq!(yos(&drain(&b), &a.user_id()), 1);
    a.sync(0).unwrap();
    for _ in 0..BURST {
        a.send_yo(pb.clone()).unwrap(); // each one fine: what the hub refuses waits
    }
    assert!(a.waiting() > 0, "over the limit");
    let started = Instant::now();
    while a.waiting() > 0 {
        assert!(started.elapsed() < Duration::from_secs(180), "the outbox empties at the hub's pace");
        std::thread::sleep(Duration::from_secs(1));
        a.deliver().unwrap();
    }
    let events = drain(&b);
    let sent: Vec<u64> = events
        .iter()
        .filter_map(|e| match e {
            YoEvent::Message { sent_at_ms, kind: MessageKind::Yo, .. } => Some(*sent_at_ms),
            _ => None,
        })
        .collect();
    assert_eq!(sent.len(), BURST, "each once");
    assert!(sent.windows(2).all(|w| w[0] <= w[1]), "in order");
}

/// A client on `disk` as the app registers it.
fn shared_client(transport: Arc<App>, disk: &Arc<Disk>, username: &str, name: Option<&str>) -> Arc<YoClient> {
    YoClient::register_username_db(
        transport,
        disk.db(),
        vec![7; 32],
        Some(Process::new(disk)),
        username.into(),
        None,
        None,
        name.map(Into::into),
        0,
        String::new(),
    )
    .unwrap()
}

/// An extension's client on the app's state: no turn to open it, each step takes its own.
fn guest_client(transport: Arc<App>, disk: &Arc<Disk>) -> Arc<YoClient> {
    YoClient::open_db(transport, disk.db(), vec![7; 32], true, Some(Process::new(disk))).unwrap()
}

#[test]
fn the_app_layer_shares_its_state_with_an_extension_in_turns() {
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (username(n), username(n + 1));
    let transport = || Arc::new(App(Http::new(hub.url())));
    let disk = Disk::new();
    let a = shared_client(transport(), &disk, &pa, Some("ALICE"));
    let b = client(&hub, &pb, "BOB", Arc::new(File::default()));
    a.send_yo(pb.clone()).unwrap();
    assert_eq!(yos(&drain(&b), &a.user_id()), 1);
    b.send_yo(pa.clone()).unwrap();

    // A share extension, while the app sleeps: it opens the newest state and sends (receiving B's Yo first); what it
    // receives stays for the app.
    let guest = |send: &dyn Fn(&YoClient) -> Vec<YoEvent>| {
        let g = guest_client(transport(), &disk);
        let events = send(&g);
        g.acknowledge_events();
        events
    };
    let photo = yo::seal_media(b"JPEG".repeat(1000), 160, 120).unwrap();
    let returned = guest(&|g| {
        let chat = g.media_chat(b.user_id()).unwrap();
        g.upload_media(photo.media.clone(), photo.ciphertext.clone(), chat.fetches).unwrap();
        let mut events = chat.events;
        events.extend(g.send_photo(b.user_id(), photo.media.clone()).unwrap());
        events.extend(g.sync(0).unwrap());
        events
    });
    assert_eq!(yos(&returned, &b.user_id()), 1, "the guest saw B's Yo");
    let at_b = drain(&b);
    assert!(at_b.iter().any(|e| matches!(e, YoEvent::Message { kind: MessageKind::Photo { .. }, .. })));

    // A notification extension: one short sync for B's next Yo, returned once; nothing new on a second sync.
    b.send_yo(pa.clone()).unwrap();
    let returned = guest(&|g| {
        let first = g.sync(0).unwrap();
        assert!(g.sync(0).unwrap().is_empty(), "returned once");
        first
    });
    assert_eq!(yos(&returned, &b.user_id()), 1);

    // The app, awake again: what the extensions saved, B's two Yos among it, handed to it at once from the state
    // (`saved_events`, no request to the hub); then it sends on from the newest state, and B opens all of it.
    let events = a.saved_events().unwrap();
    assert_eq!(yos(&events, &b.user_id()), 2, "what the guests received comes to the app");
    a.acknowledge_events();
    assert!(a.saved_events().unwrap().is_empty(), "handed once");
    assert!(a.sync(0).unwrap().is_empty(), "acknowledged");
    a.send_yo(b.user_id()).unwrap();
    a.send_yo(b.user_id()).unwrap();
    assert_eq!(yos(&drain(&b), &a.user_id()), 2);
    b.send_yo(pa.clone()).unwrap();
    assert_eq!(yos(&a.sync(0).unwrap(), &b.user_id()), 1);
    assert!(!disk.is_held(), "every turn given back");
}

/// Reads never wait for the turn: while an extension holds it (a long step), the app reads its friends, contacts and
/// groups at once, from the last committed state, and sees what the extension wrote as soon as it is written.
#[test]
fn the_app_layer_reads_while_an_extension_holds_the_turn() {
    use std::time::{Duration, Instant};
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (username(n), username(n + 1));
    let transport = || Arc::new(App(Http::new(hub.url())));
    let disk = Disk::new();
    let a = shared_client(transport(), &disk, &pa, None);
    let b = client(&hub, &pb, "BOB", Arc::new(File::default()));
    a.set_friend(b.user_id(), Some("BOB".into()), Some(3)).unwrap();
    a.send_yo(b.user_id()).unwrap();
    let g = guest_client(transport(), &disk);
    g.set_friend(b.user_id(), Some("ROBERT".into()), Some(4)).unwrap();
    // The extension in a long step: the turn held.
    let blocker = Process::new(&disk);
    blocker.hold();
    let started = Instant::now();
    let friends = a.friends();
    assert!(started.elapsed() < Duration::from_secs(2), "no wait for the turn");
    assert_eq!(friends.len(), 1);
    assert_eq!(friends[0].name.as_deref(), Some("ROBERT"), "the extension's write, taken in by a read");
    assert!(a.contacts().iter().any(|c| c.user == b.user_id()));
    assert!(a.groups().unwrap().is_empty());
    assert!(a.storage_saved_at().is_ok());
    // A write waits for the turn, and goes once it is given back.
    let writing = {
        let a = a.clone();
        let user = b.user_id();
        std::thread::spawn(move || a.set_friend(user, Some("BOBBY".into()), None))
    };
    std::thread::sleep(Duration::from_millis(300));
    assert!(!writing.is_finished(), "the write waits for the turn");
    blocker.release();
    writing.join().unwrap().unwrap();
    assert_eq!(g.friends()[0].name.as_deref(), Some("BOBBY"), "and the extension reads it without a turn");
    assert!(!disk.is_held());
}

/// An account's state from before the database (the file `yo-state`: the 36-character word of its last save, then
/// the sealed state; Yo Dev's own file without the word) moves in once and opens as the same account, its friends,
/// sessions and outbox with it.
#[test]
fn a_state_file_of_before_moves_into_the_database() {
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (username(n), username(n + 1));
    let transport = || Arc::new(App(Http::new(hub.url())));
    for word in [36usize, 0] {
        let file = Arc::new(File::default());
        let (pa, pb) = (format!("{pa}{word}"), format!("{pb}{word}"));
        let a = client(&hub, &pa, "ALICE", file.clone());
        let b = client(&hub, &pb, "BOB", Arc::new(File::default()));
        a.set_friend(b.user_id(), Some("BOB".into()), Some(2)).unwrap();
        a.send_yo(b.user_id()).unwrap();
        assert_eq!(yos(&drain(&b), &a.user_id()), 1);
        let (user, friends) = (a.user_id(), a.friends());
        a.close();
        let disk = Disk::new();
        let old = disk.dir().join("yo-state");
        let word_text = "0123456789abcdef0123456789abcdef0123";
        std::fs::write(&old, [&word_text.as_bytes()[..word], &file.0.lock().unwrap()].concat()).unwrap();
        let db = disk.db();
        assert!(!db.has_state().unwrap());
        assert!(db.move_in(old.to_string_lossy().into(), word as u32).unwrap(), "moved in");
        assert!(!old.exists(), "the file goes");
        assert!(!db.move_in(old.to_string_lossy().into(), word as u32).unwrap(), "once");
        let moved = YoClient::open_db(transport(), db, vec![7; 32], false, Some(Process::new(&disk))).unwrap();
        assert_eq!(moved.user_id(), user);
        assert_eq!(moved.friends().len(), friends.len());
        assert_eq!(moved.friends()[0].name.as_deref(), Some("BOB"));
        // Its sessions came along: both ways, no new session needed.
        moved.send_yo(b.user_id()).unwrap();
        assert_eq!(yos(&drain(&b), &user), 1);
        b.send_yo(user.clone()).unwrap();
        assert_eq!(yos(&moved.sync(0).unwrap(), &b.user_id()), 1);
        assert!(disk.word().is_some());
    }
}

#[test]
fn the_app_layer_takes_in_what_an_extension_did_while_its_long_poll_waited() {
    use std::sync::mpsc::channel;
    use std::time::Duration;
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (username(n), username(n + 1));
    let disk = Disk::new();
    let (waiting, waited) = channel();
    let (release, released) = channel();
    let held =
        Arc::new(Held { app: App(Http::new(hub.url())), waiting: Mutex::new(waiting), release: Mutex::new(released) });
    let a = YoClient::register_username_db(
        held,
        disk.db(),
        vec![7; 32],
        Some(Process::new(&disk)),
        pa.clone(),
        None,
        None,
        Some("ALICE".into()),
        0,
        String::new(),
    )
    .unwrap();
    let b = client(&hub, &pb, "BOB", Arc::new(File::default()));
    a.send_yo(pb.clone()).unwrap();
    assert_eq!(yos(&drain(&b), &a.user_id()), 1);
    let syncing = {
        let a = a.clone();
        std::thread::spawn(move || a.sync(25))
    };
    waited.recv_timeout(Duration::from_secs(30)).expect("the long poll starts");
    // While it waits (the turn is free then), B writes and the extension sends and receives.
    b.send_yo(pa.clone()).unwrap();
    let g = guest_client(Arc::new(App(Http::new(hub.url()))), &disk);
    let mut seen = g.send_yo(b.user_id()).unwrap();
    seen.extend(g.sync(0).unwrap());
    assert_eq!(yos(&seen, &b.user_id()), 1, "received before its send, once");
    drop(g);
    // The long poll's answer (B's Yo, received by the extension already) is processed against the newest state.
    release.send(()).unwrap();
    let events = syncing.join().unwrap().unwrap();
    assert_eq!(yos(&events, &b.user_id()), 1, "once, from the extension's events");
    a.acknowledge_events();
    a.send_yo(b.user_id()).unwrap();
    assert_eq!(yos(&drain(&b), &a.user_id()), 2);
    b.send_yo(pa).unwrap();
    assert_eq!(yos(&a.sync(0).unwrap(), &b.user_id()), 1);
}

#[test]
fn the_app_layer_closes_a_client_whose_shared_state_went_away() {
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (username(n), username(n + 1));
    let transport = || Arc::new(App(Http::new(hub.url())));
    let b = client(&hub, &pb, "BOB", Arc::new(File::default()));
    let disk = Disk::new();
    let a = shared_client(transport(), &disk, &pa, None);
    a.send_yo(pb.clone()).unwrap();
    // A guest opened on A's state.
    let g = guest_client(transport(), &disk);
    // Signed out, and in again as another account under the same key: the guest neither acts as it nor saves.
    disk.db().clear().unwrap();
    let other = shared_client(transport(), &disk, &username(n + 2), None);
    let saved = disk.word();
    assert!(matches!(g.send_yo(pb.clone()), Err(yo::YoError::State)));
    assert!(matches!(g.sync(0), Err(yo::YoError::State)), "closed for good");
    assert!(g.contacts().is_empty());
    assert!(matches!(a.sync(0), Err(yo::YoError::State)), "the app's old client too");
    assert_eq!(disk.word(), saved, "nothing saved over the other account's state");
    // A client whose state is gone (signed out): closed too.
    disk.db().clear().unwrap();
    assert!(matches!(other.send_yo(pb.clone()), Err(yo::YoError::State)));
    assert!(matches!(other.friends().len(), 0));
    assert_eq!(disk.word(), None, "and nothing made again");
    assert!(drain(&b).iter().all(|e| !matches!(e, YoEvent::Message { user, .. } if *user == other.user_id())));
}

/// Chaos of two processes on one account, many accounts at once: the app's long poll runs all the time, an
/// extension opens the state as a guest again and again (sends Yos, syncs like the notification service), the friend
/// writes meanwhile, and the app reads its contacts without a turn all along. Every Yo arrives exactly once, both
/// ways; every turn is given back; the state stays loadable.
#[test]
fn the_app_layer_and_its_extensions_take_turns_under_load() {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    use std::time::{Duration, Instant};
    let hub = Arc::new(TestHub::start());
    let n = random_u64() % 1_000_000_000;
    let transport = |hub: &TestHub| Arc::new(App(Http::new(hub.url())));
    let accounts: Vec<_> = (0..6u64)
        .map(|i| {
            let disk = Disk::new();
            let a = shared_client(transport(&hub), &disk, &username(n + 2 * i), None);
            let friend = client(&hub, &username(n + 2 * i + 1), "FRIEND", Arc::new(File::default()));
            a.send_yo(friend.user_id()).unwrap();
            assert_eq!(yos(&drain(&friend), &a.user_id()), 1);
            (disk, a, friend)
        })
        .collect();
    let until = Instant::now() + Duration::from_secs(20);
    let handles: Vec<_> = accounts
        .into_iter()
        .enumerate()
        .map(|(i, (disk, a, friend))| {
            let hub = hub.clone();
            std::thread::spawn(move || {
                let me = a.user_id();
                let (friend_id, sent_by_app, sent_by_guest, sent_by_friend) =
                    (friend.user_id(), AtomicUsize::new(1), AtomicUsize::new(0), AtomicUsize::new(0));
                let got_by_app = AtomicUsize::new(0);
                let got_by_friend = AtomicUsize::new(0);
                let reads = AtomicUsize::new(0);
                let guest_sent_under_load = std::sync::atomic::AtomicBool::new(false);
                std::thread::scope(|s| {
                    // The app: its long poll, and now and then a Yo of its own.
                    s.spawn(|| {
                        let mut k = 0u32;
                        while Instant::now() < until {
                            let events = a.sync(1).unwrap();
                            got_by_app.fetch_add(yos(&events, &friend_id), Relaxed);
                            a.acknowledge_events();
                            k += 1;
                            if k.is_multiple_of(3) {
                                let events = a.send_yo(friend_id.clone()).unwrap();
                                sent_by_app.fetch_add(1, Relaxed);
                                got_by_app.fetch_add(yos(&events, &friend_id), Relaxed);
                                a.acknowledge_events();
                            }
                        }
                    });
                    // The app's list: reads all along, never a turn; the friend is always there.
                    s.spawn(|| {
                        while Instant::now() < until {
                            assert!(a.contacts().iter().any(|c| c.user == friend_id), "account {i}: a read sees all");
                            reads.fetch_add(1, Relaxed);
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    });
                    // The extensions: a share sheet that sends, a notification service that syncs, each a guest.
                    s.spawn(|| {
                        let mut k = 0u64;
                        while Instant::now() < until {
                            let g = guest_client(transport(&hub), &disk);
                            if k.is_multiple_of(2) {
                                g.send_yo(friend_id.clone()).unwrap();
                                sent_by_guest.fetch_add(1, Relaxed);
                                if Instant::now() < until {
                                    guest_sent_under_load.store(true, Relaxed);
                                }
                            } else {
                                g.sync(0).unwrap();
                            }
                            drop(g);
                            k += 1;
                            std::thread::sleep(Duration::from_millis(30 + (k * 7 + i as u64) % 50));
                        }
                    });
                    // The friend writes and reads.
                    s.spawn(|| {
                        while Instant::now() < until {
                            // A send receives first: what came with it counts too.
                            got_by_friend.fetch_add(yos(&friend.send_yo(me.clone()).unwrap(), &me), Relaxed);
                            sent_by_friend.fetch_add(1, Relaxed);
                            got_by_friend.fetch_add(yos(&friend.sync(0).unwrap(), &me), Relaxed);
                            std::thread::sleep(Duration::from_millis(40));
                        }
                    });
                });
                // Quiet: everything still on its way arrives.
                for _ in 0..5 {
                    got_by_app.fetch_add(yos(&a.sync(0).unwrap(), &friend_id), Relaxed);
                    a.acknowledge_events();
                    got_by_friend.fetch_add(yos(&drain(&friend), &me), Relaxed);
                }
                let sent = sent_by_app.load(Relaxed) + sent_by_guest.load(Relaxed);
                // The first Yo was counted before the run.
                assert_eq!(got_by_friend.load(Relaxed) + 1, sent, "account {i}: the friend got every Yo once");
                assert_eq!(
                    got_by_app.load(Relaxed),
                    sent_by_friend.load(Relaxed),
                    "account {i}: the app got every Yo once"
                );
                // Opening a guest is a WAL read. A successful send needs a write turn, but its
                // rate also depends on state restoration, SQLite I/O and the hub.
                // ~80 locally; a slow CI disk (full fsync per write) gives far fewer, but never none.
                assert!(guest_sent_under_load.load(Relaxed), "account {i}: the extension sent before load ended");
                assert!(sent_by_guest.load(Relaxed) >= 3, "account {i}: the extension had turns");
                assert!(reads.load(Relaxed) > 100, "account {i}: the list read all along");
                assert!(!disk.is_held(), "account {i}: every turn given back");
                let blob = disk.db().blob().unwrap().unwrap();
                assert!(yo_core::Client::restore(Http::new(hub.url()), &blob, &[7; 32]).is_ok());
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
}

/// A file that can be full.
#[derive(Default)]
struct Full(std::sync::atomic::AtomicBool);

impl StateStore for Full {
    fn save(&self, _: Vec<u8>) -> Result<(), yo::YoError> {
        match self.0.load(std::sync::atomic::Ordering::Relaxed) {
            true => Err(yo::YoError::Save { message: "disk full".into() }),
            false => Ok(()),
        }
    }
}

/// A full disk made the media outbox's calls throw `Save`, which `media_retry_later` took for a refusal for good: the
/// app dropped the photo or video with its file. Now it is "later", and the photo goes once the state saves again.
#[test]
fn a_photo_whose_state_did_not_save_is_tried_again_later() {
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (username(n), username(n + 1));
    let disk = Arc::new(Full::default());
    let app = Arc::new(App(Http::new(hub.url())));
    let a =
        YoClient::register_username(app, disk.clone(), vec![7; 32], pa, None, None, None, 0, String::new()).unwrap();
    let _b = client(&hub, &pb, "BOB", Arc::new(File::default()));
    disk.0.store(true, std::sync::atomic::Ordering::Relaxed);
    let error = a.media_chat(pb.clone()).err().expect("not saved");
    assert!(matches!(error, yo::YoError::Save { .. }), "{error:?}");
    assert!(yo::retry_later(&error), "kept for later");
    disk.0.store(false, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(a.media_chat(pb).unwrap().fetches, 1);
}

/// A state that opens but could not be saved (a full disk) threw like one that does not open, and the app asked to
/// sign in again, which replaces the device. Now it is `Save`, which the app tries again; only a state that does not
/// open is `State`.
#[test]
fn a_state_that_opens_but_does_not_save_is_not_a_broken_one() {
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let file = Arc::new(File::default());
    let a = client(&hub, &username(n), "ANNA", file.clone());
    let blob = file.0.lock().unwrap().clone();
    let app = || Arc::new(App(Http::new(hub.url())));
    let full = Arc::new(Full(true.into()));
    let error = YoClient::restore(app(), full.clone(), vec![7; 32], blob.clone()).err().expect("not saved");
    assert!(matches!(error, yo::YoError::Save { .. }), "{error:?}");
    let error = YoClient::restore(app(), file.clone(), vec![8; 32], blob.clone()).err().expect("another key");
    assert!(matches!(error, yo::YoError::State), "{error:?}");
    full.0.store(false, std::sync::atomic::Ordering::Relaxed);
    let again = YoClient::restore(app(), full, vec![7; 32], blob).unwrap();
    assert_eq!(again.user_id(), a.user_id());
}

#[test]
fn a_closed_client_saves_nothing_and_does_nothing() {
    let hub = TestHub::start();
    let file = Arc::new(File::default());
    let a = client(&hub, &username(random_u64() % 1_000_000_000), "A", file.clone());
    let saved = file.0.lock().unwrap().clone();
    a.close();
    assert!(matches!(a.sync(0), Err(yo::YoError::State)), "every step fails");
    assert!(matches!(a.send_yo("yogpt".into()), Err(yo::YoError::State)));
    assert_eq!(*file.0.lock().unwrap(), saved, "and nothing is written over the state");
}

/// Groups through the app's layer (spec 4.9): made, a Yo to it arriving as a group Yo, the group the same everywhere.
#[test]
fn the_app_layer_makes_a_group_and_yos_it() {
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (pa, pb, pc) = (username(n), username(n + 1), username(n + 2));
    let a = client(&hub, &pa, "ALICE", Arc::new(File::default()));
    let b = client(&hub, &pb, "BOB", Arc::new(File::default()));
    let c = client(&hub, &pc, "CARL", Arc::new(File::default()));
    let id = a.create_group("FAMILY".into(), vec![b.user_id(), c.user_id()]).unwrap().id;
    for x in [&b, &c] {
        let events = drain(x);
        assert!(events.iter().any(|e| matches!(e, YoEvent::GroupChanged { group } if *group == id)));
        let group = x.groups().unwrap().into_iter().find(|g| g.id == id).unwrap();
        assert_eq!((group.name.as_str(), group.members.len(), group.left), ("FAMILY", 3, false));
    }
    a.send_group_yo(id.clone()).unwrap();
    for x in [&b, &c] {
        let got = drain(x);
        let from_a = got.iter().filter(|e| {
            matches!(e, YoEvent::GroupMessage { group, user, kind: MessageKind::Yo, .. } if *group == id && *user == a.user_id())
        });
        assert_eq!(from_a.count(), 1);
    }
    c.leave_group(id.clone()).unwrap();
    drain(&a);
    assert_eq!(a.groups().unwrap()[0].members.len(), 2);
    assert!(c.groups().unwrap()[0].left);
}

/// Background time refused: no write and no unlock, but reads go on (they need no turn), and the client can write
/// again when time is available.
#[test]
fn a_refused_shared_turn_can_be_retried() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct Retry {
        process: Arc<Process>,
        refused: AtomicBool,
        unlocks: AtomicUsize,
    }
    impl yo::StateLock for Retry {
        fn lock(&self) -> Result<(), yo::YoError> {
            if self.refused.load(Ordering::Relaxed) {
                return Err(yo::YoError::Network { message: "background time ended".into() });
            }
            yo::StateLock::lock(self.process.as_ref())
        }
        fn held(&self) -> bool {
            yo::StateLock::held(self.process.as_ref())
        }
        fn unlock(&self, saved: bool) {
            self.unlocks.fetch_add(1, Ordering::Relaxed);
            yo::StateLock::unlock(self.process.as_ref(), saved);
        }
    }

    let hub = TestHub::start();
    let disk = Disk::new();
    let lock =
        Arc::new(Retry { process: Process::new(&disk), refused: AtomicBool::new(false), unlocks: AtomicUsize::new(0) });
    let a = YoClient::register_username_db(
        Arc::new(App(Http::new(hub.url()))),
        disk.db(),
        vec![7; 32],
        Some(lock.clone()),
        username(random_u64() % 1_000_000_000),
        None,
        None,
        None,
        0,
        String::new(),
    )
    .unwrap();
    lock.refused.store(true, Ordering::Relaxed);
    let (before, unlocks) = (disk.word(), lock.unlocks.load(Ordering::Relaxed));
    assert!(matches!(a.sync(0), Err(yo::YoError::Network { .. })));
    assert!(a.groups().unwrap().is_empty(), "reads need no turn");
    assert!(a.sessions_with("contact".into()).unwrap().is_empty());
    assert!(a.storage_saved_at().is_ok());
    assert_eq!(disk.word(), before);
    assert_eq!(lock.unlocks.load(Ordering::Relaxed), unlocks);
    lock.refused.store(false, Ordering::Relaxed);
    a.sync(0).unwrap();
    assert!(lock.unlocks.load(Ordering::Relaxed) > unlocks);
    assert!(!disk.is_held());
}

/// A turn taken back in the middle of a step (background time ran out): its write never lands, and the next step
/// starts again from the database, not from what the step did in memory.
#[test]
fn a_turn_taken_back_writes_nothing_and_the_next_step_starts_from_the_database() {
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Revocable {
        process: Arc<Process>,
        revoked: AtomicBool,
    }
    impl yo::StateLock for Revocable {
        fn lock(&self) -> Result<(), yo::YoError> {
            yo::StateLock::lock(self.process.as_ref())
        }
        fn held(&self) -> bool {
            !self.revoked.load(Ordering::Relaxed) && yo::StateLock::held(self.process.as_ref())
        }
        fn unlock(&self, saved: bool) {
            yo::StateLock::unlock(self.process.as_ref(), saved);
        }
    }

    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let disk = Disk::new();
    let lock = Arc::new(Revocable { process: Process::new(&disk), revoked: AtomicBool::new(false) });
    let a = YoClient::register_username_db(
        Arc::new(App(Http::new(hub.url()))),
        disk.db(),
        vec![7; 32],
        Some(lock.clone()),
        username(n),
        None,
        None,
        None,
        0,
        String::new(),
    )
    .unwrap();
    let b = client(&hub, &username(n + 1), "BOB", Arc::new(File::default()));
    let before = disk.word();
    lock.revoked.store(true, Ordering::Relaxed);
    assert!(matches!(a.set_friend(b.user_id(), Some("BOB".into()), None), Err(yo::YoError::Save { .. })));
    assert_eq!(disk.word(), before, "nothing written");
    lock.revoked.store(false, Ordering::Relaxed);
    assert!(a.friends().is_empty(), "what the step did in memory is gone: the database's state again");
    a.set_friend(b.user_id(), Some("BOB".into()), None).unwrap();
    assert_eq!(a.friends().len(), 1);
    assert_ne!(disk.word(), before);
}

#[test]
fn roundtrip_binding_serializes_runs_and_leaves_no_contact_or_app_event() {
    use std::time::{Duration, Instant};
    let hub = TestHub::start();
    let a = client(&hub, "PROBER", "", Arc::new(File::default()));
    a.set_trust_roots(vec![yo_tests::trust_root().to_vec()]).unwrap();
    a.set_requests_open(false).unwrap();
    let mut ping = yo_tests::register(Http::new(hub.url()), "PINGTEST", None);
    let probing = a.clone();
    let worker = std::thread::spawn(move || probing.round_trip_probe("PINGTEST".into(), 3000));
    let began = Instant::now();
    let (user, token) = loop {
        ping.sync(0).unwrap();
        if let Some(request) = ping.take_probe_requests().pop() {
            break request;
        }
        assert!(began.elapsed() < Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(matches!(a.round_trip_probe("PINGTEST".into(), 3000), Err(yo::YoError::State)));
    ping.echo_probe(&user, token).unwrap();
    assert!(worker.join().unwrap().unwrap().is_some());
    assert!(a.contacts().is_empty());
    assert!(a.friends().is_empty());
    assert!(a.sync(0).unwrap().is_empty());
}

#[test]
fn the_app_layer_reads_and_marks_support_replies() {
    let config =
        yo_hub::config::Config { admin_token: Some("test-token".into()), ..yo_hub::config::Config::for_tests() };
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let a = client(&hub, &username(random_u64() % 1_000_000_000), "Test", Arc::new(File::default()));
    a.send_support("hello 🎨".into(), "1".into()).unwrap();
    let http = reqwest::blocking::Client::new();
    let threads: serde_json::Value = http
        .get(format!("{}/v1/admin/messages", hub.url()))
        .header("yo-admin-token", "test-token")
        .send()
        .unwrap()
        .json()
        .unwrap();
    let user =
        threads.as_array().unwrap().iter().find(|t| t["user_id"] != "team").unwrap()["user_id"].as_str().unwrap();
    assert_eq!(
        http.post(format!("{}/v1/admin/messages/{user}", hub.url()))
            .header("yo-admin-token", "test-token")
            .body("Hello from the team")
            .send()
            .unwrap()
            .status(),
        204
    );
    let thread = a.support_thread().unwrap();
    assert_eq!(thread.unread, 1);
    assert_eq!(thread.messages.len(), 2);
    assert!(!thread.messages[0].from_team);
    assert!(thread.messages[1].from_team);
    assert_eq!(thread.messages[0].text, "hello 🎨");
    a.support_read(thread.messages[1].id).unwrap();
    assert_eq!(a.support_thread().unwrap().unread, 0);
}

#[test]
fn warm_app_send_is_one_request_without_an_active_inbox_poll() {
    let hub = TestHub::start();
    let http = Http::new(hub.url());
    let a = YoClient::register_username(
        Arc::new(App(http.clone())),
        Arc::new(File::default()),
        vec![7; 32],
        "ALICE".into(),
        None,
        None,
        None,
        0,
        String::new(),
    )
    .unwrap();
    let mut b = yo_tests::register(Http::new(hub.url()), "BOB", None);
    a.send_yo("BOB".into()).unwrap();
    b.sync(0).unwrap();
    b.send_yo(&a.user_id()).unwrap();
    a.sync(0).unwrap(); // leave acknowledgements and housekeeping pending
    http.sent.lock().unwrap().clear();
    a.send_yo(b.user_id().to_owned()).unwrap();
    let sent = http.sent.lock().unwrap();
    assert_eq!(sent.len(), 1, "no catch-up, lookup, ack or prekey refill on send");
    assert_eq!(sent[0].0, "POST");
    assert_eq!(sent[0].1, yo_wire::api::deliver_path(&yo_tests::user_id(b.user_id())));
}
