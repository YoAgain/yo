//! yo-load: how many Yos the hub moves.
//!
//!   yo-load [--clients 200] [--seconds 15] [--raw-seconds 15] [--threads 16] [--connections 64] [--out tests/results]
//!
//! Phase 1, end to end: real clients (sessions, HTTP, the hub, fetch, decrypt) in a ring, each sending Yos to the next
//! and receiving its own. Clients and hub share this machine, so this is the whole system on one box.
//! Phase 2, the hub alone: well-formed envelopes delivered over many connections at once, as fast as it takes them.
//! The hub runs in-process on a SQLite file (WAL), as in production. Results are printed and written as JSON.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use yo_core::{Client, Content, Event, Push, Response, Transport};
use yo_hub::config::{Config, Limits};
use yo_tests::{hub_key, random_u64, username, Http, TestHub};

struct Args {
    clients: usize,
    seconds: u64,
    raw_seconds: Option<u64>,
    threads: usize,
    connections: usize,
    out: String,
}

fn args() -> Args {
    let mut args = Args {
        clients: 200,
        seconds: 15,
        raw_seconds: None,
        threads: 16,
        connections: 64,
        out: "tests/results".into(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let value = it.next().unwrap_or_default();
        match flag.as_str() {
            "--clients" => args.clients = value.parse().expect("--clients"),
            "--seconds" => args.seconds = value.parse().expect("--seconds"),
            "--raw-seconds" => args.raw_seconds = Some(value.parse().expect("--raw-seconds")),
            "--threads" => args.threads = value.parse().expect("--threads"),
            "--connections" => args.connections = value.parse().expect("--connections"),
            "--out" => args.out = value,
            other => panic!("unknown flag {other}"),
        }
    }
    args
}

// Fixed 0.1 ms buckets for the first 10 seconds, then one overflow bucket.
struct Latency {
    buckets: Vec<u64>,
    count: u64,
    max: u64,
}

impl Latency {
    const WIDTH_US: u64 = 100;
    const BUCKETS: usize = 100_001;

    fn new() -> Self {
        Self { buckets: vec![0; Self::BUCKETS], count: 0, max: 0 }
    }

    fn record(&mut self, elapsed: Duration) {
        let micros = elapsed.as_micros() as u64;
        let bucket = ((micros / Self::WIDTH_US) as usize).min(Self::BUCKETS - 1);
        self.buckets[bucket] += 1;
        self.count += 1;
        self.max = self.max.max(micros);
    }

    fn merge(&mut self, other: Self) {
        for (total, count) in self.buckets.iter_mut().zip(other.buckets) {
            *total += count;
        }
        self.count += other.count;
        self.max = self.max.max(other.max);
    }

    fn percentile(&self, p: f64) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let rank = ((self.count - 1) as f64 * p).round() as u64;
        let mut seen = 0;
        for (bucket, count) in self.buckets.iter().enumerate() {
            seen += count;
            if seen > rank {
                return (bucket as u64 * Self::WIDTH_US) as f64 / 1000.0;
            }
        }
        unreachable!("the buckets contain every sample")
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "p50_ms": self.percentile(0.50),
            "p90_ms": self.percentile(0.90),
            "p99_ms": self.percentile(0.99),
            "max_ms": self.max as f64 / 1000.0,
        })
    }
}

struct LoadHttp {
    http: Arc<Http>,
    errors: Arc<AtomicU64>,
}

impl Transport for LoadHttp {
    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String> {
        let result = self.http.request(method, path, headers, body);
        if !result.as_ref().is_ok_and(|response| response.status < 400) {
            self.errors.fetch_add(1, Ordering::Relaxed);
        }
        result
    }
}

struct Peer {
    index: usize,
    client: Client,
    from: String,
    to: String,
    sent: u64,
    received: u64,
    latest: Option<u64>,
}

impl Peer {
    fn receive(&mut self, events: Vec<Event>) -> u64 {
        let before = self.received;
        for event in events {
            if let Event::Message { user, sent_at_ms, content: Content::Yo } = event {
                assert_eq!(user, self.from, "unexpected sender at ring position {}", self.index);
                assert!(
                    self.latest.is_none_or(|last| sent_at_ms > last),
                    "duplicate or reordered Yo at {}",
                    self.index
                );
                self.latest = Some(sent_at_ms);
                self.received += 1;
            }
        }
        self.received - before
    }
}

fn main() {
    let args = args();
    let raw_seconds = args.raw_seconds.unwrap_or(args.seconds);
    let dir = std::env::temp_dir().join(format!("yo-load-{}", random_u64()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut config = Config::for_tests();
    config.db_path = dir.join("hub.sqlite").to_string_lossy().into_owned();
    // YO_LOAD_DB: another place for the hub's database (a file on disk, or ":memory:"); by default the temp dir.
    if let Ok(path) = std::env::var("YO_LOAD_DB") {
        config.db_path = path;
    }
    config.limits = Limits::unlimited();
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let url = hub.url();
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    println!(
        "yo-load: {} clients, {} s end to end, {raw_seconds} s hub alone, {} threads, {} connections, {cores} cores",
        args.clients, args.seconds, args.threads, args.connections
    );

    // ---- registration ----
    let base = random_u64() % 1_000_000_000;
    let started = Instant::now();
    let per_thread = args.clients.div_ceil(args.threads);
    let http_errors = Arc::new(AtomicU64::new(0));
    let registering: Vec<_> = (0..args.threads)
        .map(|t| {
            let url = url.clone();
            let errors = http_errors.clone();
            std::thread::spawn(move || {
                (t * per_thread..((t + 1) * per_thread))
                    .map(|i| {
                        let transport =
                            Arc::new(LoadHttp { http: Http::unrecorded(url.clone()), errors: errors.clone() });
                        let user = username(base + i as u64);
                        let mut client = Client::register_username(transport, &user, None, None, Push::none()).unwrap();
                        client.set_hub_keys(vec![hub_key()]);
                        (i, client)
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let mut clients: Vec<(usize, Client)> = registering.into_iter().flat_map(|t| t.join().unwrap()).collect();
    clients.retain(|(i, _)| *i < args.clients);
    clients.sort_by_key(|(i, _)| *i);
    let registration_s = started.elapsed().as_secs_f64();
    println!(
        "registered {} in {registration_s:.1} s ({:.0}/s, each with its prekeys)",
        clients.len(),
        clients.len() as f64 / registration_s
    );

    // ---- chats: a ring, each to the next ----
    let n = clients.len();
    let started = Instant::now();
    for (i, client) in clients.iter_mut() {
        client.send_yo(&username(base + ((*i + 1) % n) as u64)).unwrap();
    }
    for (_, client) in clients.iter_mut() {
        let events = client.sync(0).unwrap();
        assert_eq!(
            events.iter().filter(|event| matches!(event, Event::Message { content: Content::Yo, .. })).count(),
            1,
            "warm-up Yo must arrive",
        );
    }
    for (_, client) in clients.iter_mut() {
        client.after_sync().unwrap();
    }
    http_errors.store(0, Ordering::Relaxed);
    println!("{n} chats set up in {:.1} s", started.elapsed().as_secs_f64());

    // ---- phase 1: end to end ----
    let stop = Arc::new(AtomicBool::new(false));
    let sent = Arc::new(AtomicU64::new(0));
    let received = Arc::new(AtomicU64::new(0));
    let send_latency = Arc::new(Mutex::new(Latency::new()));
    let ids: Vec<_> = clients.iter().map(|(_, client)| client.user_id().to_owned()).collect();
    let mut groups: Vec<Vec<Peer>> = (0..args.threads).map(|_| Vec::new()).collect();
    for (index, client) in clients {
        let from = ids[(index + n - 1) % n].clone();
        let to = username(base + ((index + 1) % n) as u64);
        groups[index % args.threads].push(Peer { index, client, from, to, sent: 0, received: 0, latest: None });
    }
    let started = Instant::now();
    let workers: Vec<_> = groups
        .into_iter()
        .map(|mut mine| {
            let (stop, sent, received, send_latency) =
                (stop.clone(), sent.clone(), received.clone(), send_latency.clone());
            std::thread::spawn(move || {
                let mut latencies = Latency::new();
                while !stop.load(Ordering::Relaxed) {
                    for peer in &mut mine {
                        let t = Instant::now();
                        let events = peer.client.send_yo(&peer.to).unwrap();
                        latencies.record(t.elapsed());
                        peer.sent += 1;
                        sent.fetch_add(1, Ordering::Relaxed);
                        let mut count = peer.receive(events);
                        // Warm sends no longer fetch the inbox.
                        let events = peer.client.sync(0).unwrap();
                        count += peer.receive(events);
                        received.fetch_add(count, Ordering::Relaxed);
                    }
                }
                send_latency.lock().unwrap().merge(latencies);
                mine
            })
        })
        .collect();
    let duration = Duration::from_secs(args.seconds);
    while let Some(remaining) = duration.checked_sub(started.elapsed()) {
        std::thread::sleep(remaining.min(Duration::from_secs(30)));
        println!(
            "end to end at {:.0} s: {} sent, {} received, {} HTTP errors",
            started.elapsed().as_secs_f64(),
            sent.load(Ordering::Relaxed),
            received.load(Ordering::Relaxed),
            http_errors.load(Ordering::Relaxed),
        );
    }
    stop.store(true, Ordering::Relaxed);
    let mut peers: Vec<_> = workers.into_iter().flat_map(|w| w.join().unwrap()).collect();
    peers.sort_by_key(|peer| peer.index);
    let e2e_s = started.elapsed().as_secs_f64();
    // What is still in flight is received now; it counts, the time does not.
    for _ in 0..10 {
        for peer in &mut peers {
            let events = peer.client.sync(0).unwrap();
            received.fetch_add(peer.receive(events), Ordering::Relaxed);
            peer.client.after_sync().unwrap();
        }
        if sent.load(Ordering::Relaxed) == received.load(Ordering::Relaxed)
            && peers.iter().all(|peer| peer.client.waiting() == 0)
        {
            break;
        }
    }
    let waiting: usize = peers.iter().map(|peer| peer.client.waiting()).sum();
    let e2e_errors = http_errors.load(Ordering::Relaxed);
    let dropped: u64 = peers.iter().map(|peer| peer.client.dropped()).sum();
    let (sent, received) = (sent.load(Ordering::Relaxed), received.load(Ordering::Relaxed));
    let e2e = serde_json::json!({
        "seconds": e2e_s,
        "yos_sent": sent,
        "yos_received": received,
        "dropped": dropped,
        "waiting": waiting,
        "http_errors": e2e_errors,
        "yos_per_s": sent as f64 / e2e_s,
        "send": send_latency.lock().unwrap().json(),
    });
    println!("end to end: {sent} Yos sent, {received} received, {dropped} dropped, {:.0} Yos/s", sent as f64 / e2e_s);

    // ---- phase 2: the hub alone ----
    // Each user with its device check, learned once as any sender would (spec 4.4), and its access key, as a contact
    // shows it (spec 4.10): a delivery without it is a request, refused unsigned.
    let targets: Vec<(String, String, Option<String>)> = peers
        .iter()
        .map(|peer| {
            let c = &peer.client;
            let access = c.access_key_for_tests();
            let hash = yo_tests::devices_hash_with(&url, c.user_id(), access.as_deref()).expect("a registered user");
            (yo_wire::api::deliver_path(&yo_tests::user_id(c.user_id())), yo_wire::b64(&hash), access)
        })
        .collect();
    let envelope = yo_tests::junk(0x5A);
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let raw = runtime.block_on(async {
        let client = reqwest::Client::builder().pool_max_idle_per_host(args.connections).build().unwrap();
        let deadline = Instant::now() + Duration::from_secs(raw_seconds);
        let tasks: Vec<_> = (0..args.connections)
            .map(|c| {
                let (client, url, envelope, targets) = (client.clone(), url.clone(), envelope.clone(), targets.clone());
                tokio::spawn(async move {
                    let mut latencies = Latency::new();
                    let mut errors = 0u64;
                    let mut k = c;
                    while Instant::now() < deadline {
                        let (path, hash, access) = &targets[k % targets.len()];
                        k += 1;
                        let t = Instant::now();
                        let mut request =
                            client.post(format!("{url}{path}")).header(yo_wire::api::HEADER_DEVICES, hash.as_str());
                        if let Some(access) = access {
                            request = request.header(yo_wire::api::HEADER_ACCESS, access.as_str());
                        }
                        let request = request.body(envelope.clone());
                        match request.send().await {
                            Ok(r) if r.status().as_u16() == 202 => latencies.record(t.elapsed()),
                            _ => errors += 1,
                        }
                    }
                    (latencies, errors)
                })
            })
            .collect();
        let started = Instant::now();
        let mut all = Latency::new();
        let mut errors = 0;
        for task in tasks {
            let (latencies, e) = task.await.unwrap();
            all.merge(latencies);
            errors += e;
        }
        (all, errors, started.elapsed().as_secs_f64())
    });
    let (latencies, errors, raw_s) = raw;
    let delivered = latencies.count;
    let raw = serde_json::json!({
        "connections": args.connections,
        "seconds": raw_s,
        "delivered": delivered,
        "errors": errors,
        "per_s": delivered as f64 / raw_s,
        "latency": latencies.json(),
    });
    println!("hub alone: {delivered} deliveries, {errors} errors, {:.0}/s", delivered as f64 / raw_s);

    let result = serde_json::json!({
        "date": std::env::var("YO_DATE").unwrap_or_default(),
        "commit": std::env::var("YO_COMMIT").unwrap_or_default(),
        "cores": cores,
        "clients": n,
        "threads": args.threads,
        "registration": { "seconds": registration_s, "per_s": n as f64 / registration_s },
        "end_to_end": e2e,
        "hub_alone": raw,
    });
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
    std::fs::create_dir_all(&args.out).unwrap();
    let name = format!("{}/{}-{}c.json", args.out, std::env::var("YO_DATE").unwrap_or_else(|_| "run".into()), n);
    std::fs::write(&name, serde_json::to_string_pretty(&result).unwrap() + "\n").unwrap();
    println!("written to {name}");
    hub.stop();
    let _ = std::fs::remove_dir_all(dir);
    for peer in &peers {
        let receiver = &peers[(peer.index + 1) % n];
        assert_eq!(peer.sent, receiver.received, "mismatched Yo count on ring hop {}", peer.index);
    }
    assert_eq!((dropped, waiting, e2e_errors, errors), (0, 0, 0, 0), "load left drops, pending messages or errors");
}
