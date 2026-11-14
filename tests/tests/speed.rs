//! How fast a Yo goes: from the sender's tap (the app layer's `send_yo`, with the inbox's long wait running as in the
//! app) to the hub's push request, and what the notification service does for one push (requests, time). Prints its
//! numbers; the bounds are loose (a shared CI machine), the numbers are what the tests/README table shows.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use yo::{HttpResult, HttpTransport, StateStore, YoClient};
use yo_hub::config::{ApnsConfig, Config};
use yo_tests::{random_u64, username, Http, TestHub};
use yo_wire::api::PUSH_APNS;

/// The app's HTTP: counts the requests and remembers their routes.
struct App {
    http: Arc<Http>,
    routes: Mutex<Vec<String>>,
}

impl App {
    fn new(hub: &TestHub) -> Arc<Self> {
        Arc::new(App { http: Http::new(hub.url()), routes: Mutex::new(Vec::new()) })
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.routes.lock().unwrap())
    }
}

impl HttpTransport for App {
    fn request(&self, method: String, path: String, headers: Vec<yo::Header>, body: Vec<u8>) -> HttpResult {
        let route: String = path.split('/').take(3).collect::<Vec<_>>().join("/");
        self.routes.lock().unwrap().push(format!("{method} {}", route.split('?').next().unwrap_or("")));
        let headers: Vec<(String, String)> = headers.into_iter().map(|h| (h.name, h.value)).collect();
        match yo_core::Transport::request(self.http.as_ref(), &method, &path, &headers, &body) {
            Ok(r) => HttpResult { status: r.status, body: r.body, error: None },
            Err(e) => HttpResult { status: 0, body: Vec::new(), error: Some(e) },
        }
    }
}

#[derive(Default)]
struct File {
    blob: Mutex<Vec<u8>>,
    saves: AtomicUsize,
}

impl StateStore for File {
    fn save(&self, blob: Vec<u8>) {
        self.saves.fetch_add(1, Ordering::Relaxed);
        *self.blob.lock().unwrap() = blob;
    }
}

fn hub() -> TestHub {
    let mut config = Config::for_tests();
    config.apns = Some(ApnsConfig {
        key_pem: include_str!("../fixtures/test-apns.p8").into(),
        key_id: "KEYID12345".into(),
        team_id: "TEAMID1234".into(),
        topic: "co.justyo.app".into(),
    });
    TestHub::start_with(config, "127.0.0.1:0")
}

fn client(app: Arc<App>, file: Arc<File>, name: &str, token: &str) -> Arc<YoClient> {
    let platform = if token.is_empty() { 0 } else { PUSH_APNS };
    YoClient::register_username(app, file, vec![7; 32], name.into(), None, None, platform, token.into()).unwrap()
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn percentiles(mut values: Vec<f64>) -> String {
    values.sort_by(f64::total_cmp);
    let at = |p: f64| values[((values.len() - 1) as f64 * p).round() as usize];
    format!("p50 {:.1} ms, p90 {:.1} ms, max {:.1} ms", at(0.5), at(0.9), at(1.0))
}

/// The time from `send_yo` to the push request at the hub, one Yo at a time and ten a second, while the sender's app
/// waits at its inbox as the list does.
#[test]
fn a_yo_is_pushed_within_milliseconds_of_its_send() {
    let hub = hub();
    let n = random_u64() % 1_000_000_000;
    let (name_a, name_b) = (username(n), username(n + 1));
    let file_a = Arc::new(File::default());
    let a = client(App::new(&hub), file_a.clone(), &name_a, "");
    let b = client(App::new(&hub), Arc::new(File::default()), &name_b, "a1b2c3");
    a.send_yo(name_b.clone()).unwrap();
    b.sync(0).unwrap();
    // The list's long wait, as in the app (Inbox.listen): sends need not receive first meanwhile.
    let waiting = a.clone();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped = stop.clone();
    let listener = std::thread::spawn(move || {
        while !stopped.load(Ordering::Relaxed) {
            let _ = waiting.sync(1);
        }
    });
    std::thread::sleep(Duration::from_millis(200));
    let pushed = || hub.push.sent.lock().unwrap().len();
    let mut send = Vec::new();
    let mut push = Vec::new();
    let saves = file_a.saves.load(Ordering::Relaxed);
    for i in 0..60 {
        let before = pushed();
        let start = Instant::now();
        a.send_yo(b.user_id()).unwrap();
        send.push(ms(start.elapsed()));
        while pushed() == before {
            assert!(start.elapsed() < Duration::from_secs(5), "pushed");
            std::thread::sleep(Duration::from_micros(200));
        }
        push.push(ms(start.elapsed()));
        // The first 30 alone, the rest at ten a second.
        std::thread::sleep(Duration::from_millis(if i < 30 { 300 } else { 100 }).saturating_sub(start.elapsed()));
    }
    stop.store(true, Ordering::Relaxed);
    listener.join().unwrap();
    let saves = file_a.saves.load(Ordering::Relaxed) - saves;
    println!("send_yo: {}, {:.1} saves of the state each", percentiles(send.clone()), saves as f64 / 60.0);
    println!("send_yo to push request: {}", percentiles(push.clone()));
    push.sort_by(f64::total_cmp);
    assert!(push[push.len() * 9 / 10] < 250.0, "{push:?}");
}

/// The notification service's work for one push (NotificationService.newest): the state opened as a guest, the
/// inbox read without waiting, the names. Its requests, and how long it took on this machine.
#[test]
fn the_notification_service_reads_a_push_in_few_requests() {
    let hub = hub();
    let n = random_u64() % 1_000_000_000;
    let names: Vec<String> = (0..21).map(|i| username(n + i)).collect();
    let file = Arc::new(File::default());
    let app = App::new(&hub);
    let b = client(app.clone(), file.clone(), &names[0], "a1b2c3");
    // Twenty friends with chats in use, as a phone has after a while.
    let friends: Vec<_> =
        names[1..].iter().map(|name| client(App::new(&hub), Arc::new(File::default()), name, "")).collect();
    for friend in &friends {
        friend.send_yo(names[0].clone()).unwrap();
    }
    b.sync(0).unwrap();
    for friend in &friends {
        b.send_yo(friend.user_id()).unwrap();
        friend.sync(0).unwrap();
    }
    b.sync(0).unwrap();
    let blob = file.blob.lock().unwrap().clone();
    println!("state: {} KiB", blob.len() / 1024);
    let start = Instant::now();
    let saves = file.saves.load(Ordering::Relaxed);
    for _ in 0..20 {
        let _ = YoClient::restore(App::new(&hub), Arc::new(File::default()), vec![7; 32], blob.clone()).unwrap();
    }
    println!("open the state: {:.1} ms", ms(start.elapsed()) / 20.0);
    assert_eq!(file.saves.load(Ordering::Relaxed), saves);
    let mut took = Vec::new();
    let mut requests = Vec::new();
    for friend in friends.iter().take(10) {
        friend.send_yo(b.user_id()).unwrap();
        let blob = file.blob.lock().unwrap().clone();
        let guest_app = App::new(&hub);
        let start = Instant::now();
        let guest = YoClient::restore_guest(guest_app.clone(), file.clone(), vec![7; 32], blob).unwrap();
        let events = guest.sync(0).unwrap();
        let _ = guest.contacts();
        took.push(ms(start.elapsed()));
        assert_eq!(events.len(), 1);
        requests = guest_app.take();
        // The app takes it in as ever.
        drop(guest);
        let app_blob = file.blob.lock().unwrap().clone();
        let again = YoClient::restore(App::new(&hub), file.clone(), vec![7; 32], app_blob).unwrap();
        again.sync(0).unwrap();
        again.acknowledge_events();
    }
    println!("notification service: {}, requests {requests:?}", percentiles(took));
    let _ = app.take();
    assert!(requests.len() <= 3, "{requests:?}");
}
