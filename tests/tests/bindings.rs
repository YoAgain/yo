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

#[test]
fn the_app_layer_sends_to_one_friend_while_a_delivery_to_another_waits() {
    use std::sync::mpsc::channel;
    use std::time::Duration;
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (pa, pb, pc) = (phone(n), phone(n + 1), phone(n + 2));
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
    yo::request_code(gate.clone(), pa.clone()).unwrap();
    let a = YoClient::register(
        gate,
        Arc::new(File::default()),
        vec![7; 32],
        pa.clone(),
        "000000".into(),
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
    let pa = phone(n);
    let slow = Arc::new(Slow(App(Http::new(hub.url()))));
    yo::request_code(slow.clone(), pa.clone()).unwrap();
    let a = YoClient::register(
        slow,
        Arc::new(File::default()),
        vec![7; 32],
        pa.clone(),
        "000000".into(),
        None,
        0,
        "".into(),
    )
    .unwrap();
    let friends: Vec<(String, Arc<YoClient>)> = (1..=FRIENDS)
        .map(|i| {
            let p = phone(n + i);
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
    let got = Arc::new(Mutex::new(Vec::<YoEvent>::new()));
    let stop = Arc::new(AtomicBool::new(false));
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
    const BURST: usize = 70;
    let mut config = yo_hub::config::Config::for_tests();
    config.limits.deliver_per_ip_minute = 60;
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (phone(n), phone(n + 1));
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
        assert!(started.elapsed() < Duration::from_secs(90), "the outbox empties at the hub's pace");
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
