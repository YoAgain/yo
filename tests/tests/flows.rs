//! Every 1:1 flow end to end through the layer the apps call (`yo::YoClient`, UniFFI), against a hub in-process:
//! the app's HTTP and its file as callbacks, faults and crashes where the app meets them.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use yo::{HttpResult, HttpTransport, MessageKind, StateStore, YoClient, YoError, YoEvent};
use yo_tests::{deliver_raw, random_u64, username, Fault, Http, TestHub};

const KEY: [u8; 32] = [7; 32];

/// The app's HTTP: the harness's, with its faults; and a crash, once told: the process dies right after the
/// request to a path reached the hub (its file keeps what was saved until then).
struct App(Arc<Http>, Mutex<Option<(String, Arc<File>)>>);

impl App {
    fn new(http: Arc<Http>) -> Arc<App> {
        Arc::new(App(http, Mutex::default()))
    }
}

impl HttpTransport for App {
    fn request(&self, method: String, path: String, headers: Vec<yo::Header>, body: Vec<u8>) -> HttpResult {
        let headers: Vec<(String, String)> = headers.into_iter().map(|h| (h.name, h.value)).collect();
        let result = yo_core::Transport::request(self.0.as_ref(), &method, &path, &headers, &body);
        let mut crash = self.1.lock().unwrap();
        if crash.as_ref().is_some_and(|(pattern, _)| path.contains(pattern.as_str())) {
            let (_, file) = crash.take().unwrap();
            file.frozen.store(true, Ordering::Relaxed);
            return HttpResult { status: 0, body: Vec::new(), error: Some("crashed".into()) };
        }
        match result {
            Ok(r) => HttpResult { status: r.status, body: r.body, error: None },
            Err(e) => HttpResult { status: 0, body: Vec::new(), error: Some(e) },
        }
    }
}

/// The app's file: the sealed state as saved last; frozen, it keeps what it had (the process died then), full, it
/// throws.
#[derive(Default)]
struct File {
    blob: Mutex<Vec<u8>>,
    frozen: AtomicBool,
    full: AtomicBool,
}

impl StateStore for File {
    fn save(&self, blob: Vec<u8>) -> Result<(), YoError> {
        if self.full.load(Ordering::Relaxed) {
            return Err(YoError::Save { message: "disk full".into() });
        }
        if !self.frozen.load(Ordering::Relaxed) {
            *self.blob.lock().unwrap() = blob;
        }
        Ok(())
    }
}

impl File {
    fn saved(&self) -> Vec<u8> {
        self.blob.lock().unwrap().clone()
    }
}

/// A phone: the client, its HTTP and its file.
struct Phone {
    client: Arc<YoClient>,
    app: Arc<App>,
    http: Arc<Http>,
    file: Arc<File>,
    name: String,
}

impl std::ops::Deref for Phone {
    type Target = YoClient;
    fn deref(&self) -> &YoClient {
        &self.client
    }
}

impl Phone {
    fn new(hub: &TestHub, name: &str, shown: Option<&str>) -> Phone {
        Phone::register(hub, name, shown, None)
    }

    fn register(hub: &TestHub, name: &str, shown: Option<&str>, secret: Option<Vec<u8>>) -> Phone {
        let http = Http::new(hub.url());
        let app = App::new(http.clone());
        let file = Arc::new(File::default());
        let client = YoClient::register_username(
            app.clone(),
            file.clone(),
            KEY.to_vec(),
            name.into(),
            secret,
            shown.map(str::to_owned),
            0,
            String::new(),
        )
        .unwrap();
        Phone { client, app, http, file, name: name.into() }
    }

    /// The process dies right after the next request to `path` reached the hub.
    fn crash_after(&self, path: &str) {
        *self.app.1.lock().unwrap() = Some((path.into(), self.file.clone()));
    }

    /// The app killed and started again from what its file holds.
    fn restart(&self, hub: &TestHub) -> Phone {
        self.file.frozen.store(false, Ordering::Relaxed);
        let http = Http::new(hub.url());
        let app = App::new(http.clone());
        let file = Arc::new(File::default());
        let client = YoClient::restore(app.clone(), file.clone(), KEY.to_vec(), self.file.saved()).unwrap();
        Phone { client, app, http, file, name: self.name.clone() }
    }

    fn user(&self) -> String {
        self.client.user_id()
    }

    /// Syncs and does the work after, until nothing more comes; the events, acknowledged.
    fn drain(&self) -> Vec<YoEvent> {
        let mut all = Vec::new();
        for _ in 0..4 {
            let events = self.sync(0).unwrap();
            self.acknowledge_events();
            self.after_sync().unwrap();
            all.extend(events);
        }
        all
    }
}

fn pair(hub: &TestHub) -> (Phone, Phone) {
    let n = random_u64() % 1_000_000_000;
    (Phone::new(hub, &username(n), Some("ALICE")), Phone::new(hub, &username(n + 1), Some("BOB")))
}

/// A and B with sessions both ways.
fn friends(hub: &TestHub) -> (Phone, Phone) {
    let (a, b) = pair(hub);
    a.send_yo(b.name.clone()).unwrap();
    assert_eq!(yos(&b.drain(), &a.user()), 1);
    b.send_yo(a.user()).unwrap();
    assert_eq!(yos(&a.drain(), &b.user()), 1);
    (a, b)
}

fn yos(events: &[YoEvent], from: &str) -> usize {
    events.iter().filter(|e| matches!(e, YoEvent::Message { user, kind: MessageKind::Yo, .. } if user == from)).count()
}

/// Events as words, for a failure's message.
fn describe(events: &[YoEvent]) -> String {
    let one = |e: &YoEvent| match e {
        YoEvent::NewChat { user } => format!("new chat {}", &user[..8]),
        YoEvent::Message { user, kind, .. } => format!("message {} {}", &user[..8], kind_name(kind)),
        YoEvent::SentElsewhere { user, .. } => format!("sent elsewhere {}", &user[..8]),
        YoEvent::Profile { user, name } => format!("profile {} {name}", &user[..8]),
        YoEvent::IdentityChanged { user } => format!("identity {}", &user[..8]),
        YoEvent::GroupMessage { group, user, kind, .. } => {
            format!("group {} message {} {}", &group[..8], &user[..8], kind_name(kind))
        }
        YoEvent::GroupSentElsewhere { group, .. } => format!("group {} sent elsewhere", &group[..8]),
        YoEvent::GroupChanged { group } => format!("group {} changed", &group[..8]),
    };
    events.iter().map(one).collect::<Vec<_>>().join(", ")
}

fn kind_name(kind: &MessageKind) -> &'static str {
    match kind {
        MessageKind::Yo => "yo",
        MessageKind::Photo { .. } => "photo",
        MessageKind::Video { .. } => "video",
        _ => "other",
    }
}

fn yo_times(events: &[YoEvent], from: &str) -> Vec<u64> {
    events
        .iter()
        .filter_map(|e| match e {
            YoEvent::Message { user, sent_at_ms, kind: MessageKind::Yo } if user == from => Some(*sent_at_ms),
            _ => None,
        })
        .collect()
}

/// The envelopes posted to a user's inbox, in order, whether the hub got them or not.
fn posted(http: &Http, to: &str) -> Vec<Vec<u8>> {
    let path = yo_wire::api::deliver_path(&yo_tests::user_id(to));
    http.sent.lock().unwrap().iter().filter(|(m, p, _)| m == "POST" && *p == path).map(|(_, _, b)| b.clone()).collect()
}

/// What an envelope to `to` carries, opened with its account secret.
fn payload(to: &Phone, envelope: &[u8]) -> yo_wire::envelope::Payload {
    yo_wire::decode(&yo_core::seal::open(&to.account_secret().try_into().unwrap(), envelope).unwrap()).unwrap()
}

#[test]
fn register_add_a_friend_and_yo_both_ways() {
    let hub = TestHub::start();
    let (a, b) = pair(&hub);
    // A adds B by the name typed: asked of the hub, kept as a friend.
    let found = a.whois(b.name.clone()).unwrap().expect("B is on Yo");
    assert_eq!(found.user, b.user());
    a.set_friend(b.user(), Some("Bob".into()), Some(3)).unwrap();
    a.write_storage().unwrap();
    assert!(a.whois(username(999_999_999)).unwrap().is_none());
    a.send_yo(b.name.clone()).unwrap();
    let events = b.drain();
    assert!(events.contains(&YoEvent::NewChat { user: a.user() }));
    assert!(events.contains(&YoEvent::Profile { user: a.user(), name: "ALICE".into() }));
    assert_eq!(yos(&events, &a.user()), 1);
    b.send_yo(a.user()).unwrap();
    let events = a.drain();
    assert_eq!(yos(&events, &b.user()), 1);
    assert!(events.contains(&YoEvent::Profile { user: b.user(), name: "BOB".into() }));
    assert!(!events.iter().any(|e| matches!(e, YoEvent::NewChat { .. })), "A wrote first: no new chat for A");
    for (me, peer) in [(&a, &b), (&b, &a)] {
        let sessions = me.sessions_with(peer.user());
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].usable && !sessions[0].retrying);
        assert_eq!(me.safety_number(peer.user()).unwrap(), peer.safety_number(me.user()).unwrap());
    }
    assert_eq!(a.friends().iter().map(|f| f.user.clone()).collect::<Vec<_>>(), [b.user()]);
    assert_eq!(b.contacts()[0].handles, std::slice::from_ref(&a.name), "known by the name its owner writes");
    assert_eq!(a.waiting() + b.waiting(), 0);
}

#[test]
fn a_photo_and_a_video_go_as_blobs_and_are_opened_once() {
    let hub = TestHub::start();
    let (a, b) = friends(&hub);
    let photo = b"HEIC".repeat(20_000);
    let video = b"MP4!".repeat(100_000);
    let (sealed_photo, sealed_video) =
        (yo::seal_media(photo.clone(), 640, 480).unwrap(), yo::seal_media(video.clone(), 1080, 1920).unwrap());
    let chat = a.media_chat(b.user()).unwrap();
    assert_eq!(chat.fetches, 1);
    a.upload_media(sealed_photo.media.clone(), sealed_photo.ciphertext.clone(), chat.fetches).unwrap();
    a.upload_media(sealed_video.media.clone(), sealed_video.ciphertext.clone(), chat.fetches).unwrap();
    a.send_photo(b.user(), sealed_photo.media.clone()).unwrap();
    a.send_video(b.user(), sealed_video.media.clone(), 4_500).unwrap();
    let events = b.drain();
    let (mut photos, mut videos) = (Vec::new(), Vec::new());
    for event in events {
        match event {
            YoEvent::Message { kind: MessageKind::Photo { media }, .. } => photos.push(media),
            YoEvent::Message { kind: MessageKind::Video { media, duration_ms }, .. } => {
                videos.push((media, duration_ms))
            }
            _ => {}
        }
    }
    assert_eq!((photos.len(), videos.len()), (1, 1));
    assert!(photos[0] == sealed_photo.media && videos[0].0 == sealed_video.media && videos[0].1 == 4_500);
    assert_eq!(b.fetch_media(photos[0].clone()).unwrap(), Some(photo));
    assert_eq!(b.fetch_media(videos[0].0.clone()).unwrap(), Some(video));
    assert_eq!(b.fetch_media(photos[0].clone()).unwrap(), None, "fetched by B's one device: gone");
    // The ciphertext is all the hub holds: a key that is not the photo's opens nothing.
    let mut wrong = sealed_photo.media.clone();
    wrong.key = vec![0; 32];
    let video_blob = sealed_video.media.clone();
    assert!(a.fetch_media(video_blob).unwrap().is_none(), "gone after its fetch");
    assert!(b.fetch_media(wrong).unwrap().is_none());
    // A photo whose blob is not there: its Yo still arrives, the fetch finds nothing.
    let missing = yo::seal_media(b"JPEG".to_vec(), 1, 1).unwrap();
    a.send_photo(b.user(), missing.media.clone()).unwrap();
    let events = b.drain();
    assert!(events.iter().any(|e| matches!(e, YoEvent::Message { kind: MessageKind::Photo { .. }, .. })));
    assert_eq!(b.fetch_media(missing.media).unwrap(), None);
}

#[test]
fn many_yos_fast_arrive_each_once_in_order_both_ways() {
    const BURST: usize = 150;
    let hub = Arc::new(TestHub::start());
    let (a, b) = friends(&hub);
    let (a, b) = (Arc::new(a), Arc::new(b));
    let sender = |from: Arc<Phone>, to: String| {
        std::thread::spawn(move || {
            for _ in 0..BURST {
                from.send_yo(to.clone()).unwrap();
            }
        })
    };
    // Both send while both receive: the long polls and the bursts share the clients.
    let receiver = |me: Arc<Phone>, from: String| {
        std::thread::spawn(move || {
            let mut times = Vec::new();
            let started = std::time::Instant::now();
            while times.len() < BURST && started.elapsed() < std::time::Duration::from_secs(120) {
                let events = me.sync(1).unwrap();
                me.acknowledge_events();
                me.after_sync().unwrap();
                times.extend(yo_times(&events, &from));
            }
            times
        })
    };
    let (ra, rb) = (receiver(a.clone(), b.user()), receiver(b.clone(), a.user()));
    let (sa, sb) = (sender(a.clone(), b.user()), sender(b.clone(), a.user()));
    sa.join().unwrap();
    sb.join().unwrap();
    for (received, who) in [(ra.join().unwrap(), "A"), (rb.join().unwrap(), "B")] {
        assert_eq!(received.len(), BURST, "{who}: each once");
        assert!(received.windows(2).all(|w| w[0] < w[1]), "{who}: in order");
    }
    assert_eq!(a.waiting() + b.waiting(), 0);
}

/// One envelope never reaches the hub, one twice (its answer lost, sent again), one late after the next one:
/// what arrives is shown once, in the ratchet's order of what opened, and the session goes on.
#[test]
fn lost_duplicated_and_reordered_envelopes() {
    let hub = TestHub::start();
    let (a, b) = friends(&hub);
    let to_b = yo_wire::api::deliver_path(&yo_tests::user_id(&b.user()));
    // Lost: A is told the hub took it.
    a.http.fault(&to_b, Fault::Answer(202));
    a.send_yo(b.user()).unwrap();
    // Twice: the hub took it, the answer was lost; A sends it again.
    a.http.fault(&to_b, Fault::LoseAnswer);
    a.send_yo(b.user()).unwrap();
    assert_eq!(a.waiting(), 1, "not known to have arrived");
    a.deliver().unwrap();
    assert_eq!(a.waiting(), 0);
    // Late: held back, posted by hand after the next one; and the next one once more.
    a.http.fault(&to_b, Fault::Answer(202));
    a.send_yo(b.user()).unwrap();
    a.send_yo(b.user()).unwrap();
    let sent = posted(&a.http, &b.user());
    let (late, next) = (sent[sent.len() - 2].clone(), sent[sent.len() - 1].clone());
    assert_eq!(yos(&b.drain(), &a.user()), 2, "the doubled one once, and the next");
    assert_eq!(deliver_raw(&hub.url(), &b.user(), late), 202);
    assert_eq!(deliver_raw(&hub.url(), &b.user(), next), 202);
    assert_eq!(yos(&b.drain(), &a.user()), 1, "the late one, the copy skipped");
    // Both ways still.
    b.send_yo(a.user()).unwrap();
    assert_eq!(yos(&a.drain(), &b.user()), 1);
    a.send_yo(b.user()).unwrap();
    assert_eq!(yos(&b.drain(), &a.user()), 1);
}

fn hub_on_file() -> (yo_hub::config::Config, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("yo-flows-{}", random_u64()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut config = yo_hub::config::Config::for_tests();
    config.db_path = dir.join("hub.sqlite").to_string_lossy().into_owned();
    (config, dir)
}

/// The hub dies while a Yo is on its way (it took it, the answer never came), and while the next waits; started
/// again on its database (another port), everything arrives once.
#[test]
fn a_hub_restart_in_the_middle_of_a_send_loses_and_repeats_nothing() {
    let (config, dir) = hub_on_file();
    let hub = TestHub::start_with(config.clone(), "127.0.0.1:0");
    let (a, b) = friends(&hub);
    let to_b = yo_wire::api::deliver_path(&yo_tests::user_id(&b.user()));
    a.http.fault(&to_b, Fault::LoseAnswer);
    a.send_yo(b.user()).unwrap();
    assert_eq!(a.waiting(), 1);
    hub.stop();
    // Down: the Yo by user id waits in the outbox; by name it needs the hub.
    a.send_yo(b.user()).unwrap();
    assert!(matches!(a.send_yo(b.name.clone()), Err(YoError::Network { .. })));
    assert!(matches!(b.sync(0), Err(YoError::Network { .. })));
    assert_eq!(a.waiting(), 2);
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    for phone in [&a, &b] {
        *phone.http.base.write().unwrap() = hub.url();
    }
    a.deliver().unwrap();
    assert_eq!(a.waiting(), 0);
    let times = yo_times(&b.drain(), &a.user());
    assert_eq!(times.len(), 2, "the one the hub took before it died once, the one that waited");
    b.send_yo(a.user()).unwrap();
    assert_eq!(yos(&a.drain(), &b.user()), 1);
    drop(hub);
    let _ = std::fs::remove_dir_all(dir);
}

/// The app dies at each point of a send and a receive: before its save (the disk full: nothing left), after the
/// save before the hub took it (it goes after the restart), after the hub took it before the state knew (it goes
/// again, shown once), after events were saved before the app showed them (they come again, once).
#[test]
fn a_crash_before_or_after_a_save_loses_nothing_and_shows_nothing_twice() {
    let hub = TestHub::start();
    let (a, b) = friends(&hub);
    let to_b = yo_wire::api::deliver_path(&yo_tests::user_id(&b.user()));
    // Before the save: nothing leaves, the call says so.
    a.file.full.store(true, Ordering::Relaxed);
    assert!(matches!(a.send_yo(b.user()), Err(YoError::Save { .. })));
    a.file.full.store(false, Ordering::Relaxed);
    assert_eq!(yos(&b.drain(), &a.user()), 0, "nothing left unsaved");
    // Saved, then the process dies before the hub takes it: the restarted app sends it.
    a.http.fault(&to_b, Fault::Answer(503));
    a.send_yo(b.user()).unwrap();
    assert_eq!(a.waiting(), 1);
    let a = a.restart(&hub);
    assert_eq!(a.waiting(), 1, "the outbox was saved");
    a.deliver().unwrap();
    assert_eq!(yos(&b.drain(), &a.user()), 1);
    // The hub takes it and the process dies before its state notes that: it goes again after the restart, once.
    a.crash_after(&to_b);
    a.send_yo(b.user()).unwrap();
    let a = a.restart(&hub);
    a.deliver().unwrap();
    assert_eq!(a.waiting(), 0);
    assert_eq!(yos(&b.drain(), &a.user()), 1, "the repeat skipped");
    // B saves what arrived and dies before the app has it: it comes again after the restart, and only then.
    a.send_yo(b.user()).unwrap();
    assert_eq!(yos(&b.sync(0).unwrap(), &a.user()), 1);
    let b = b.restart(&hub);
    assert_eq!(yos(&b.sync(0).unwrap(), &a.user()), 1, "handed again: the app never had it");
    b.acknowledge_events();
    b.after_sync().unwrap();
    let b = b.restart(&hub);
    assert_eq!(yos(&b.drain(), &a.user()), 0, "acknowledged: never again");
    // A frozen B (it died right after the hub handed out its inbox, nothing saved): the hub sends it again.
    a.send_yo(b.user()).unwrap();
    b.file.frozen.store(true, Ordering::Relaxed);
    assert_eq!(yos(&b.sync(0).unwrap(), &a.user()), 1);
    let b = b.restart(&hub);
    assert_eq!(yos(&b.drain(), &a.user()), 1, "not acknowledged unsaved: again");
    // And the sessions survived all of it.
    b.send_yo(a.user()).unwrap();
    assert_eq!(yos(&a.drain(), &b.user()), 1);
    a.send_yo(b.user()).unwrap();
    assert_eq!(yos(&b.drain(), &a.user()), 1);
}

/// A new phone from the backup: opened by the words, registered with the account secret it holds, friends back from
/// the storage; the old phone is out; the friend's sessions form with the new device on its next delivery, both ways.
#[test]
fn a_new_phone_from_the_words_gets_its_friends_back_and_sessions_form_again() {
    let hub = TestHub::start();
    let (a, b) = friends(&hub);
    a.set_friend(b.user(), Some("Bob".into()), Some(2)).unwrap();
    a.write_storage().unwrap();
    let (k, words) = (vec![1; 32], yo_tests::backup::words([9; 16]));
    let json = format!("{{\"secret\":{:?}}}", a.account_secret());
    a.put_backup(k.clone(), json.clone().into_bytes(), words.clone(), yo::YoPasskeyDoor::None).unwrap();
    // The new phone: nothing but the words.
    let app = App::new(Http::new(hub.url()));
    let opened = yo::open_backup(app, None, yo::YoDoor::Words { words }).unwrap().expect("the record");
    assert_eq!((opened.k, String::from_utf8(opened.sealed).unwrap()), (k, json));
    let a2 = Phone::register(&hub, &a.name, None, Some(a.account_secret()));
    assert_eq!(a2.user(), a.user());
    assert_eq!(a2.username(), Some(a.name.clone()));
    let friends = a2.friends();
    assert_eq!(friends.len(), 1);
    assert_eq!(
        (friends[0].user.clone(), friends[0].name.clone(), friends[0].color),
        (b.user(), Some("Bob".into()), Some(2))
    );
    assert!(matches!(a.sync(0), Err(YoError::Replaced)));
    assert!(matches!(a.send_yo(b.user()), Err(YoError::Replaced)));
    // B writes to the account as it knew it: the hub names the new device, B starts a session with it.
    b.send_yo(a.user()).unwrap();
    assert_eq!(b.waiting(), 0);
    let events = a2.drain();
    assert_eq!(yos(&events, &b.user()), 1);
    assert!(!events.iter().any(|e| matches!(e, YoEvent::IdentityChanged { .. })));
    a2.send_yo(b.user()).unwrap();
    let events = b.drain();
    assert_eq!(yos(&events, &a.user()), 1);
    assert!(
        !events.iter().any(|e| matches!(e, YoEvent::IdentityChanged { .. } | YoEvent::NewChat { .. })),
        "the same account: no new chat, no new key"
    );
    let sessions = b.sessions_with(a.user());
    assert_eq!(sessions.len(), 1, "the old device's session is gone");
    assert!(sessions[0].usable);
    b.send_yo(a.name.clone()).unwrap();
    assert_eq!(yos(&a2.drain(), &b.user()), 1);
}

/// The account on a new phone, the old one still sending: what it had waiting goes out unsigned (deliveries are),
/// and the friend shows nothing of it once it knows the new device. Its own calls are refused (`Replaced`).
#[test]
fn a_replaced_device_still_sending_is_not_heard() {
    let hub = TestHub::start();
    let (a, b) = friends(&hub);
    let to_b = yo_wire::api::deliver_path(&yo_tests::user_id(&b.user()));
    // A Yo of the old phone held back, posted later by hand (it can still deliver).
    a.http.fault(&to_b, Fault::Answer(202));
    a.send_yo(b.user()).unwrap();
    let held = posted(&a.http, &b.user()).pop().unwrap();
    // A Yo of the old phone waiting in its outbox while the account moves.
    a.http.fault(&to_b, Fault::Answer(503));
    a.send_yo(b.user()).unwrap();
    assert_eq!(a.waiting(), 1);
    let a2 = Phone::register(&hub, &a.name, None, Some(a.account_secret()));
    a2.send_yo(b.user()).unwrap();
    let events = b.drain();
    assert_eq!(yos(&events, &a.user()), 1, "the new phone's Yo");
    // The old phone comes back online.
    let _ = a.deliver();
    assert!(matches!(a.sync(0), Err(YoError::Replaced)));
    assert!(matches!(a.send_yo(b.user()), Err(YoError::Replaced)));
    assert_eq!(deliver_raw(&hub.url(), &b.user(), held), 202);
    let events = b.drain();
    assert_eq!(yos(&events, &a.user()), 0, "nothing of the replaced device: {}", describe(&events));
    // B and the new phone go on.
    b.send_yo(a.user()).unwrap();
    assert_eq!(yos(&a2.drain(), &b.user()), 1);
    assert_eq!(b.sessions_with(a.user()).len(), 1);
}

/// B's state is rolled back past its session with A (a backup of the file from before): A's next Yo does not open.
/// B asks again (a silent retry in a new session); A sends again what it keeps; B shows what it had not, once.
#[test]
fn a_wedged_session_heals_by_a_retry() {
    let hub = TestHub::start();
    let (a, b) = pair(&hub);
    let early = b.file.saved();
    a.send_yo(b.user()).unwrap();
    assert_eq!(yos(&b.drain(), &a.user()), 1);
    b.send_yo(a.user()).unwrap();
    assert_eq!(yos(&a.drain(), &b.user()), 1);
    *b.file.blob.lock().unwrap() = early;
    let b = b.restart(&hub);
    a.send_yo(b.user()).unwrap();
    let events = b.sync(0).unwrap();
    assert_eq!(yos(&events, &a.user()), 0, "it does not open");
    b.acknowledge_events();
    b.after_sync().unwrap();
    assert!(b.sessions_with(a.user()).iter().all(|s| !s.retrying), "the retry went");
    a.drain();
    let times = yo_times(&b.drain(), &a.user());
    // The first (shown before the rollback, which B's file forgot) and the one that did not open.
    assert_eq!(times.len(), 2, "what A keeps, once each");
    b.send_yo(a.user()).unwrap();
    assert_eq!(yos(&a.drain(), &b.user()), 1);
    a.send_yo(b.user()).unwrap();
    assert_eq!(yos(&b.drain(), &a.user()), 1);
}

/// B's one-time prekeys used up at the hub (lookups that never led to a message): the next sender's session is made
/// with the last resort, and it opens; a second one too; then B refills.
#[test]
fn exhausted_prekeys_fall_back_to_the_last_resort() {
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let b = Phone::new(&hub, &username(n), Some("BOB"));
    let mut stranger =
        yo_core::Client::register_username(Http::new(hub.url()), &username(n + 1), None, None, yo_core::Push::none())
            .unwrap();
    let lookup = yo_wire::encode(&yo_wire::api::Lookup {
        handle: yo_wire::handle::Handle::user(&yo_tests::user_id(&b.user())),
        flags: yo_wire::api::LOOKUP_PREKEYS,
        except: Vec::new(),
    });
    let mut last = None;
    for _ in 0..yo_wire::limits::PREKEY_TARGET + 2 {
        let response = stranger.signed_for_tests("POST", yo_wire::api::LOOKUP, &lookup).unwrap();
        assert_eq!(response.status, 200);
        last = Some(yo_wire::decode::<yo_wire::api::LookupResult>(&response.body).unwrap());
    }
    let bundle = &last.unwrap().bundles[0];
    assert!(bundle.one_time.is_none(), "none left");
    let senders: Vec<Phone> = (2..4).map(|i| Phone::new(&hub, &username(n + i), None)).collect();
    for sender in &senders {
        sender.send_yo(b.user()).unwrap();
    }
    let events = b.drain();
    for sender in &senders {
        assert_eq!(yos(&events, &sender.user()), 1, "made with the last resort, opened");
        b.send_yo(sender.user()).unwrap();
        assert_eq!(yos(&sender.drain(), &b.user()), 1);
    }
    // Refilled: the next lookup has a one-time prekey again.
    let response = stranger.signed_for_tests("POST", yo_wire::api::LOOKUP, &lookup).unwrap();
    let found = yo_wire::decode::<yo_wire::api::LookupResult>(&response.body).unwrap();
    assert!(found.bundles[0].one_time.is_some(), "refilled");
}

#[test]
fn a_blocked_friend_is_not_heard_and_not_written_to() {
    let hub = TestHub::start();
    let (a, b) = friends(&hub);
    b.set_blocked(a.user(), true).unwrap();
    a.send_yo(b.user()).unwrap();
    assert_eq!(yos(&b.drain(), &a.user()), 0, "not heard");
    assert!(matches!(b.send_yo(a.user()), Err(YoError::Blocked)));
    assert_eq!(yos(&a.drain(), &b.user()), 0);
    assert!(b.contacts().iter().any(|c| c.user == a.user() && c.blocked));
    assert!(b.friends().iter().any(|f| f.user == a.user() && f.blocked), "kept in the storage");
    b.set_blocked(a.user(), false).unwrap();
    a.send_yo(b.user()).unwrap();
    assert_eq!(yos(&b.drain(), &a.user()), 1, "heard again, only what came after");
    b.send_yo(a.user()).unwrap();
    assert_eq!(yos(&a.drain(), &b.user()), 1);
}

#[test]
fn a_deleted_account_is_gone_for_its_friend_and_its_name_free() {
    let hub = TestHub::start();
    let (a, b) = friends(&hub);
    a.delete_account().unwrap();
    assert!(matches!(a.sync(0), Err(YoError::Replaced)));
    assert!(yo::username_free(App::new(Http::new(hub.url())), a.name.clone()).unwrap());
    assert!(matches!(b.send_yo(a.user()), Err(YoError::NotOnYo)));
    assert!(b.contacts().iter().any(|c| c.user == a.user() && c.gone));
    assert_eq!(b.waiting(), 0, "nothing waits for nobody");
    assert!(matches!(b.send_yo(a.name.clone()), Err(YoError::NotOnYo)));
    // Someone new takes the name: a stranger to B, written to as such.
    let c = Phone::new(&hub, &a.name, Some("CAROL"));
    assert_ne!(c.user(), a.user());
    b.send_yo(a.name.clone()).unwrap();
    let events = c.drain();
    assert_eq!(yos(&events, &b.user()), 1);
    assert!(events.contains(&YoEvent::NewChat { user: b.user() }));
}

/// Silent receipts (spec 4.5): B never sends anything, yet each sync of B answers A's Yos with a receipt, so A's
/// session is acknowledged and its ratchet moves: A's Yos stop being prekey messages after B's first receipt.
#[test]
fn silent_receipts_heal_a_one_way_stream() {
    let hub = TestHub::start();
    let (a, b) = pair(&hub);
    let kinds = |a: &Phone, b: &Phone| -> Vec<u8> {
        posted(&a.http, &b.user()).iter().map(|e| payload(b, e).copies[0].prekey).collect()
    };
    a.send_yo(b.user()).unwrap();
    a.send_yo(b.user()).unwrap();
    assert_eq!(kinds(&a, &b), [1, 1, 1], "the name and two Yos, unacknowledged: prekey messages");
    assert_eq!(yos(&b.drain(), &a.user()), 2);
    assert!(b.contacts().iter().any(|c| c.user == a.user()));
    // A takes in B's receipt: nothing to show.
    let events = a.drain();
    assert!(events.iter().all(|e| matches!(e, YoEvent::Profile { .. })), "{}", describe(&events));
    let ratchets = |a: &Phone, b: &Phone| -> Vec<Vec<u8>> {
        posted(&a.http, &b.user())
            .iter()
            .map(|e| payload(b, e).copies.remove(0))
            .filter(|c| c.prekey == 0)
            .map(|c| {
                // A SignalMessage: its version, then its protobuf, whose field 1 is the sender's ratchet key.
                let m = c.message.as_slice();
                assert_eq!((m[1], m[2]), (0x0a, 33));
                m[3..36].to_vec()
            })
            .collect()
    };
    let mut keys = Vec::new();
    for round in 0..3 {
        a.send_yo(b.user()).unwrap();
        assert_eq!(yos(&b.drain(), &a.user()), 1, "round {round}");
        a.drain();
        keys = ratchets(&a, &b);
    }
    assert_eq!(*kinds(&a, &b).last().unwrap(), 0, "acknowledged: plain session messages");
    let mut distinct = keys.clone();
    distinct.dedup();
    assert_eq!(distinct.len(), keys.len(), "a new ratchet key each round: the stream heals");
    assert!(b.sent_nothing_shown(&a));
}

trait Quiet {
    fn sent_nothing_shown(&self, to: &Phone) -> bool;
}

impl Quiet for Phone {
    /// Nothing this phone sent showed at `to` as content.
    fn sent_nothing_shown(&self, to: &Phone) -> bool {
        to.drain().iter().all(|e| !matches!(e, YoEvent::Message { user, .. } if *user == self.user()))
    }
}

/// What an envelope shows anyone but its recipient: a bucket of random-looking bytes; nothing of who sent it, to
/// which device, or what (a Yo and a text look alike), and nothing repeats between two of the same.
#[test]
fn sealed_bytes_reveal_nothing() {
    let hub = TestHub::start();
    let (a, b) = friends(&hub);
    // The first ones of the session carry its PQXDH (a larger bucket, as any session's first): those after count.
    let before = posted(&a.http, &b.user()).len();
    a.send_yo(b.user()).unwrap();
    a.send_yo(b.user()).unwrap();
    let sealed_photo = yo::seal_media(b"JPEG".repeat(100), 1, 1).unwrap();
    a.send_photo(b.user(), sealed_photo.media.clone()).unwrap();
    let envelopes = posted(&a.http, &b.user()).split_off(before);
    assert_eq!(envelopes.len(), 3);
    let sizes: std::collections::BTreeSet<usize> = envelopes.iter().map(Vec::len).collect();
    assert_eq!(sizes.len(), 1, "one bucket for a Yo and a photo: {sizes:?}");
    let a_account =
        ed25519_dalek::SigningKey::from_bytes(&a.account_secret().try_into().unwrap()).verifying_key().to_bytes();
    let b_account =
        ed25519_dalek::SigningKey::from_bytes(&b.account_secret().try_into().unwrap()).verifying_key().to_bytes();
    let a_payload = payload(&b, &envelopes[0]);
    let secrets: Vec<(&str, Vec<u8>)> = vec![
        ("A's user id", yo_tests::user_id(&a.user()).to_vec()),
        ("B's user id", yo_tests::user_id(&b.user()).to_vec()),
        ("A's account key", a_account.to_vec()),
        ("B's account key", b_account.to_vec()),
        ("A's device key", a_payload.device_key.to_vec()),
        ("B's device key", a_payload.copies[0].device_key.to_vec()),
        ("A's name", a.name.clone().into_bytes()),
        ("A's shown name", b"ALICE".to_vec()),
        ("the blob id", sealed_photo.media.id.clone()),
        ("the blob key", sealed_photo.media.key.clone()),
    ];
    for envelope in &envelopes {
        assert_eq!(envelope[0], yo_wire::envelope::SEALED);
        for (what, bytes) in &secrets {
            assert!(!envelope.windows(bytes.len()).any(|w| w == bytes.as_slice()), "{what} in an envelope");
        }
        // No long run of padding zeros, nothing shared with another envelope beyond chance.
        assert!(!envelope.windows(16).any(|w| w.iter().all(|&x| x == 0)), "padding in the clear");
    }
    for (i, x) in envelopes.iter().enumerate() {
        for y in &envelopes[i + 1..] {
            let same = x.windows(12).step_by(12).filter(|w| y.windows(12).any(|v| v == *w)).count();
            assert_eq!(same, 0, "two envelopes share bytes");
        }
    }
    // The state's file too: sealed under the app's key, nothing in the clear.
    let saved = a.file.saved();
    for (what, bytes) in &secrets {
        assert!(!saved.windows(bytes.len()).any(|w| w == bytes.as_slice()), "{what} in the saved state");
    }
}

/// The client's clock moved on by `ms` (the hub's and libsignal's stay).
fn later(phone: &Phone, ms: u64) {
    let at = yo_hub::now_ms() + ms;
    phone.core_for_tests(|c| c.set_clock(Arc::new(move || at)));
}

const HOUR: u64 = 3600 * 1000;
const DAY: u64 = 24 * HOUR;

/// B's device as an app from before sessions has it at the hub: without prekeys. The closure brings them back, as
/// the update uploads them.
fn old_app(config: &yo_hub::config::Config, b: &Phone) -> impl FnOnce() {
    let db = rusqlite::Connection::open(&config.db_path).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
    let device: Vec<u8> = db
        .query_row("SELECT id FROM devices WHERE user_id = ?1", [yo_tests::user_id(&b.user()).to_vec()], |r| r.get(0))
        .unwrap();
    let rows: Vec<(i64, i64, Vec<u8>, Vec<u8>)> = db
        .prepare("SELECT kind, id, key, signature FROM prekeys WHERE device_id = ?1")
        .unwrap()
        .query_map([&device], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    db.execute("DELETE FROM prekeys WHERE device_id = ?1", [&device]).unwrap();
    move || {
        for (kind, id, key, signature) in &rows {
            db.execute(
                "INSERT INTO prekeys (device_id, kind, id, key, signature) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![&device, kind, id, key, signature],
            )
            .unwrap();
        }
    }
}

/// A friend whose app is too old for sessions (its device has no prekeys at the hub): the Yo waits, `Outdated`, the
/// friend shows "needs update"; once its prekeys are there, the outbox goes, each Yo once, in order.
#[test]
fn a_friend_who_must_update_gets_what_waited_once_prekeys_appear() {
    let (config, dir) = hub_on_file();
    let hub = TestHub::start_with(config.clone(), "127.0.0.1:0");
    let (a, b) = pair(&hub);
    let updated = old_app(&config, &b);
    assert!(matches!(a.send_yo(b.name.clone()), Err(YoError::Outdated)));
    assert!(matches!(a.send_yo(b.user()), Err(YoError::Outdated)));
    assert!(a.contacts().iter().any(|c| c.user == b.user() && c.outdated));
    assert_eq!(a.waiting(), 3, "the name and both Yos wait");
    assert_eq!(yos(&b.drain(), &a.user()), 0);
    // Updated: its prekeys are there. Asked again at most hourly.
    updated();
    later(&a, HOUR + 1000);
    a.deliver().unwrap();
    assert_eq!(a.waiting(), 0, "the outbox went");
    assert!(a.contacts().iter().all(|c| !c.outdated));
    let events = b.drain();
    let times = yo_times(&events, &a.user());
    assert_eq!(times.len(), 2, "each once: {}", describe(&events));
    assert!(times[0] < times[1]);
    b.send_yo(a.user()).unwrap();
    assert_eq!(yos(&a.drain(), &b.user()), 1);
    a.send_yo(b.user()).unwrap();
    assert_eq!(yos(&b.drain(), &a.user()), 1);
    drop(hub);
    let _ = std::fs::remove_dir_all(dir);
}

/// B away 30 days (its app not run, its prekeys replaced weekly before): what waited at the hub opens, the session
/// goes on both ways; A, whose clock moved as far, too.
#[test]
fn thirty_days_offline_then_everything_arrives() {
    let hub = TestHub::start();
    let (a, b) = pair(&hub);
    a.send_yo(b.user()).unwrap();
    let mut events = Vec::new();
    for week in 1..=4 {
        later(&b, week * 7 * DAY + HOUR);
        events.extend(b.maintenance().unwrap());
        b.acknowledge_events();
    }
    assert_eq!(yos(&events, &a.user()), 1, "the first one, while B was still about");
    a.send_yo(b.user()).unwrap();
    a.send_yo(b.user()).unwrap();
    hub.hub.prune(yo_hub::now_ms() + 29 * DAY).unwrap();
    later(&b, 4 * 7 * DAY + HOUR + 30 * DAY);
    let events = b.drain();
    assert_eq!(yos(&events, &a.user()), 2, "{}", describe(&events));
    b.send_yo(a.user()).unwrap();
    later(&a, 30 * DAY);
    assert_eq!(yos(&a.drain(), &b.user()), 1);
    a.send_yo(b.user()).unwrap();
    assert_eq!(yos(&b.drain(), &a.user()), 1);
    for phone in [&a, &b] {
        let (dropped, why) = phone.core_for_tests(|c| (c.dropped(), c.drop_reasons().to_vec()));
        assert_eq!(dropped, 0, "{why:?}");
    }
}

/// The state's file shared by the app and its notification service: the sealed state with how often it was saved,
/// and the lock between them.
#[derive(Default)]
struct Disk {
    state: Mutex<(u64, Vec<u8>)>,
    held: Mutex<bool>,
    free: std::sync::Condvar,
}

/// One process's view of `Disk`: which save it read or made last.
struct Process {
    disk: Arc<Disk>,
    known: Mutex<u64>,
}

impl Process {
    fn new(disk: &Arc<Disk>) -> Arc<Self> {
        Arc::new(Process { disk: disk.clone(), known: Mutex::new(0) })
    }

    fn hold(&self) {
        let mut held = self.disk.held.lock().unwrap();
        while *held {
            held = self.disk.free.wait(held).unwrap();
        }
        *held = true;
    }

    fn release(&self) {
        *self.disk.held.lock().unwrap() = false;
        self.disk.free.notify_all();
    }

    fn read(&self) -> Vec<u8> {
        let state = self.disk.state.lock().unwrap();
        *self.known.lock().unwrap() = state.0;
        state.1.clone()
    }
}

impl StateStore for Process {
    fn save(&self, blob: Vec<u8>) -> Result<(), YoError> {
        assert!(*self.disk.held.lock().unwrap(), "saved only under the lock");
        let mut state = self.disk.state.lock().unwrap();
        *state = (state.0 + 1, blob);
        *self.known.lock().unwrap() = state.0;
        Ok(())
    }
}

impl yo::StateLock for Process {
    fn lock(&self) -> yo::StateTurn {
        self.hold();
        let state = self.disk.state.lock().unwrap();
        let mut known = self.known.lock().unwrap();
        if *known == state.0 {
            return yo::StateTurn::Same;
        }
        *known = state.0;
        if state.1.is_empty() {
            yo::StateTurn::Gone
        } else {
            yo::StateTurn::Newer { blob: state.1.clone() }
        }
    }

    fn unlock(&self) {
        self.release();
    }
}

/// The notification service opens the app's state as a guest while the app has it open (in memory, between
/// calls): it decrypts the pushed Yos and returns them for the notification; the app, at its next call, takes in
/// the newer state (the session moved on), shows the Yos once and goes on writing in the same sessions.
#[test]
fn the_notification_service_decrypts_while_the_app_holds_the_state() {
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let disk = Arc::new(Disk::default());
    let app = Process::new(&disk);
    app.hold();
    let a = YoClient::register_username(
        App::new(Http::new(hub.url())),
        app.clone(),
        KEY.to_vec(),
        username(n),
        None,
        Some("ALICE".into()),
        0,
        String::new(),
    )
    .unwrap();
    app.release();
    a.share_state(app.clone());
    let b = Phone::new(&hub, &username(n + 1), Some("BOB"));
    a.send_yo(b.user()).unwrap();
    assert_eq!(yos(&b.drain(), &a.user_id()), 1);
    for round in 0..3 {
        b.send_yo(a.user_id()).unwrap();
        b.send_yo(a.user_id()).unwrap();
        // The push: the extension's own process, holding the lock for as long as it runs.
        let nse = Process::new(&disk);
        nse.hold();
        let guest =
            YoClient::restore_guest(App::new(Http::new(hub.url())), nse.clone(), KEY.to_vec(), nse.read()).unwrap();
        let shown = guest.sync(0).unwrap();
        guest.after_sync().unwrap();
        guest.acknowledge_events();
        drop(guest);
        nse.release();
        assert_eq!(yos(&shown, &b.user()), 2, "round {round}: decrypted by the extension");
        // The app comes back: what the extension received, once.
        let events = a.saved_events().unwrap();
        a.acknowledge_events();
        assert_eq!(yos(&events, &b.user()), 2, "round {round}: handed to the app");
        assert!(a.sync(0).unwrap().is_empty(), "round {round}: nothing twice");
        a.acknowledge_events();
        a.after_sync().unwrap();
        a.send_yo(b.user()).unwrap();
        assert_eq!(yos(&b.drain(), &a.user_id()), 1, "round {round}: the app writes on");
    }
    assert_eq!(a.core_for_tests(|c| c.dropped()), 0);
    assert_eq!(b.core_for_tests(|c| c.dropped()), 0);
}

/// The friend who had to update writes first once it has: its session is there, so what waited goes with the next
/// delivery, and the friend is no longer "needs update", nor is a Yo to it `Outdated`.
#[test]
fn a_friend_who_updated_and_writes_first_is_no_longer_outdated() {
    let (config, dir) = hub_on_file();
    let hub = TestHub::start_with(config.clone(), "127.0.0.1:0");
    let (a, b) = pair(&hub);
    let updated = old_app(&config, &b);
    assert!(matches!(a.send_yo(b.user()), Err(YoError::Outdated)));
    updated();
    b.send_yo(a.user()).unwrap();
    assert_eq!(yos(&a.drain(), &b.user()), 1);
    assert_eq!(a.waiting(), 0, "what waited went with the session B's Yo made");
    assert!(a.contacts().iter().all(|c| !c.outdated), "no longer needs an update");
    a.send_yo(b.user()).unwrap();
    assert_eq!(yos(&b.drain(), &a.user()), 2);
    drop(hub);
    let _ = std::fs::remove_dir_all(dir);
}

/// A Yo the old phone sent before the account moved, taken by the hub, still in the friend's inbox when the friend
/// writes to the account and learns of the new phone (the hub's `409`): it arrives, once. Its device is gone by then,
/// but was not when it sent.
#[test]
fn a_yo_the_old_phone_sent_before_the_move_arrives_after_the_friend_learned_of_it() {
    let hub = TestHub::start();
    let (a, b) = friends(&hub);
    a.send_yo(b.user()).unwrap();
    let a2 = Phone::register(&hub, &a.name, None, Some(a.account_secret()));
    let mut events = b.send_yo(a.user()).unwrap();
    assert_eq!(yos(&a2.drain(), &b.user()), 1);
    events.extend(b.drain());
    assert_eq!(yos(&events, &a.user()), 1, "{}", describe(&events));
    a2.send_yo(b.user()).unwrap();
    assert_eq!(yos(&b.drain(), &a.user()), 1);
}

/// As above, the inbox not readable just then (`503`): the roster waits with the Yo to the account, and the old
/// phone's Yo still opens once the inbox is read.
#[test]
fn the_roster_waits_while_the_inbox_cannot_be_read_first() {
    let hub = TestHub::start();
    let (a, b) = friends(&hub);
    a.send_yo(b.user()).unwrap();
    let a2 = Phone::register(&hub, &a.name, None, Some(a.account_secret()));
    b.http.fault("/v1/inbox?wait=0", Fault::Answer(503));
    let mut events = b.send_yo(a.user()).unwrap_or_default();
    events.extend(b.drain());
    assert_eq!(yos(&events, &a.user()), 1, "{}", describe(&events));
    assert_eq!(b.waiting(), 0);
    assert_eq!(yos(&a2.drain(), &b.user()), 1);
}
