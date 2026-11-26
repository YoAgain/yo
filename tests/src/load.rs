//! yo-load: how many Yos the hub moves.
//!
//!   yo-load [--clients 200] [--seconds 15] [--raw-seconds 15] [--threads 16] [--connections 64] [--client-threads 16]
//!           [--out tests/results]
//!
//! Phase 1, end to end: real clients (sessions, HTTP, the hub, fetch, decrypt) in a ring, each sending Yos to the next
//! and receiving its own. Clients and hub share this machine, so this is the whole system on one box.
//! Phase 2, the hub alone: well-formed envelopes delivered over many connections at once, as fast as it takes them.
//! Phase 3, with `YO_CLIENT_CPU=n` only, one client core: `--client-threads` threads on CPU n alone send Yos over the
//! ring's established sessions through the core's whole send path (encrypt, sealed sender, saving the client's
//! encrypted state file, HTTP), nothing received; everything else (the hub too) runs on the other CPUs. Each client
//! then saves to its own state file, and the ring's first Yos (prekey fetch, PQXDH) are sent from CPU n too.
//! The hub runs in-process on a SQLite file (WAL), as in production. Results are printed and written as JSON.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use yo_core::statedb::{DbStore, StateDb};
use yo_core::{Client, Content, Event, Push, Response, Transport};
use yo_hub::config::{Config, Limits};
use yo_tests::{affinity, hub_key, random_u64, set_affinity, username, Http, TestHub};

/// The state files' key: a measurement's own, thrown away with them.
const STATE_KEY: [u8; 32] = [7; 32];

struct Args {
    clients: usize,
    seconds: u64,
    raw_seconds: Option<u64>,
    threads: usize,
    connections: usize,
    client_threads: usize,
    out: String,
}

fn args() -> Args {
    let mut args = Args {
        clients: 200,
        seconds: 15,
        raw_seconds: None,
        threads: 16,
        connections: 64,
        client_threads: 16,
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
            "--client-threads" => args.client_threads = value.parse().expect("--client-threads"),
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
    send_started: mpsc::Sender<Instant>,
    receive_started: mpsc::Receiver<Instant>,
}

impl Peer {
    fn receive(&mut self, events: Vec<Event>, latency: &mut Latency) -> u64 {
        let decrypted = Instant::now();
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
                let started = self.receive_started.try_recv().expect("a send started for every decrypted Yo");
                latency.record(decrypted.duration_since(started));
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
    // YO_CLIENT_CPU=n: CPU n is kept for the one-core send phase; the hub and the rest of the load get the others.
    let client_cpu = std::env::var("YO_CLIENT_CPU").ok().and_then(|cpu| cpu.parse::<usize>().ok());
    let all_cpus = affinity();
    let other_cpus: Vec<_> = all_cpus.iter().copied().filter(|cpu| Some(*cpu) != client_cpu).collect();
    if let Some(cpu) = client_cpu {
        assert!(all_cpus.contains(&cpu), "YO_CLIENT_CPU must be in the allowed CPU set");
        assert!(!other_cpus.is_empty(), "YO_CLIENT_CPU needs another CPU for the hub");
        assert!(std::env::var("YO_HUB_CPU").ok() != Some(cpu.to_string()), "YO_CLIENT_CPU and YO_HUB_CPU must differ");
        set_affinity(&other_cpus); // the hub's threads inherit it
    }
    // The clients' state files, next to the hub's database (on the same disk).
    let states = std::env::var("YO_LOAD_DB")
        .ok()
        .and_then(|db| std::path::Path::new(&db).parent().map(|parent| parent.to_path_buf()))
        .filter(|parent| parent.is_dir())
        .unwrap_or_else(|| dir.clone())
        .join(format!("yo-load-clients-{}", random_u64()));
    if client_cpu.is_some() {
        std::fs::create_dir_all(&states).unwrap();
    }
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
            let states = client_cpu.map(|_| states.clone());
            std::thread::spawn(move || {
                (t * per_thread..((t + 1) * per_thread))
                    .map(|i| {
                        let transport =
                            Arc::new(LoadHttp { http: Http::unrecorded(url.clone()), errors: errors.clone() });
                        let user = username(base + i as u64);
                        let mut client = Client::register_username(transport, &user, None, None, Push::none()).unwrap();
                        client.set_hub_keys(vec![hub_key()]);
                        if let Some(states) = &states {
                            let path = states.join(format!("{i}.sqlite"));
                            let db = Arc::new(StateDb::open_fresh(path, STATE_KEY, Arc::new(|_| {})).unwrap());
                            client.persist_to(Arc::new(DbStore::new(db, None, None)), STATE_KEY).unwrap();
                        }
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
    // Each first Yo to a username: its lookup, prekey fetch and PQXDH, on the client CPU when there is one.
    let mut first_latency = Latency::new();
    if let Some(cpu) = client_cpu {
        set_affinity(&[cpu]);
    }
    for (i, client) in clients.iter_mut() {
        let t = Instant::now();
        client.send_yo(&username(base + ((*i + 1) % n) as u64)).unwrap();
        first_latency.record(t.elapsed());
    }
    if client_cpu.is_some() {
        set_affinity(&other_cpus);
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
    // Each ring hop is FIFO. Publish the monotonic start before sending, since the recipient
    // can decrypt the Yo before the sender gets its HTTP response.
    let (starts, mut arrivals): (Vec<_>, Vec<_>) = (0..n).map(|_| mpsc::channel()).unzip();
    arrivals.rotate_right(1);
    for (((index, client), send_started), receive_started) in clients.into_iter().zip(starts).zip(arrivals) {
        let from = ids[(index + n - 1) % n].clone();
        let to = username(base + ((index + 1) % n) as u64);
        groups[index % args.threads].push(Peer {
            index,
            client,
            from,
            to,
            sent: 0,
            received: 0,
            latest: None,
            send_started,
            receive_started,
        });
    }
    let started = Instant::now();
    let workers: Vec<_> = groups
        .into_iter()
        .map(|mut mine| {
            let (stop, sent, received, send_latency) =
                (stop.clone(), sent.clone(), received.clone(), send_latency.clone());
            std::thread::spawn(move || {
                let mut latencies = Latency::new();
                let mut decrypted_latencies = Latency::new();
                while !stop.load(Ordering::Relaxed) {
                    for peer in &mut mine {
                        let t = Instant::now();
                        peer.send_started.send(t).unwrap();
                        let events = peer.client.send_yo(&peer.to).unwrap();
                        latencies.record(t.elapsed());
                        peer.sent += 1;
                        sent.fetch_add(1, Ordering::Relaxed);
                        let mut count = peer.receive(events, &mut decrypted_latencies);
                        // Warm sends no longer fetch the inbox.
                        let events = peer.client.sync(0).unwrap();
                        count += peer.receive(events, &mut decrypted_latencies);
                        received.fetch_add(count, Ordering::Relaxed);
                    }
                }
                send_latency.lock().unwrap().merge(latencies);
                (mine, decrypted_latencies)
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
    let completed: Vec<_> = workers.into_iter().map(|worker| worker.join().unwrap()).collect();
    let e2e_s = started.elapsed().as_secs_f64();
    let mut peers = Vec::with_capacity(n);
    let mut yo_to_yo = Latency::new();
    for (group, latency) in completed {
        peers.extend(group);
        yo_to_yo.merge(latency);
    }
    peers.sort_by_key(|peer| peer.index);
    // What is still in flight is received now; it counts, the time does not.
    for _ in 0..10 {
        for peer in &mut peers {
            let events = peer.client.sync(0).unwrap();
            received.fetch_add(peer.receive(events, &mut yo_to_yo), Ordering::Relaxed);
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
    assert_eq!(yo_to_yo.count, received);
    let e2e = serde_json::json!({
        "seconds": e2e_s,
        "yos_sent": sent,
        "yos_received": received,
        "dropped": dropped,
        "waiting": waiting,
        "http_errors": e2e_errors,
        "yos_per_s": sent as f64 / e2e_s,
        "send": send_latency.lock().unwrap().json(),
        "yo_to_yo": yo_to_yo.json(),
    });
    println!("end to end: {sent} Yos sent, {received} received, {dropped} dropped, {:.0} Yos/s", sent as f64 / e2e_s);
    println!("Yo to Yo: p50 {:.1} ms, p99 {:.1} ms", yo_to_yo.percentile(0.50), yo_to_yo.percentile(0.99));

    // ---- phase 2: the hub alone ----
    // Each user with its device check, learned once as any sender would (spec 3.2), and its access key, as a contact
    // shows it (spec 3.3): a delivery without it is a request, refused unsigned.
    let targets: Vec<(String, String, Option<String>)> = peers
        .iter()
        .map(|peer| {
            let c = &peer.client;
            let access = c.access_key_for_tests();
            let hash = yo_tests::devices_hash_with(&url, c.user_id(), access.as_deref()).expect("a registered user");
            (yoip::net::api::deliver_path(&yo_tests::user_id(c.user_id())), yoip::b64(&hash), access)
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
                            client.post(format!("{url}{path}")).header(yoip::net::api::HEADER_DEVICES, hash.as_str());
                        if let Some(access) = access {
                            request = request.header(yoip::net::api::HEADER_ACCESS, access.as_str());
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

    // ---- phase 3: one client core sending ----
    let one_core = client_cpu.map(|cpu| {
        http_errors.store(0, Ordering::Relaxed);
        let stop = AtomicBool::new(false);
        let per_thread = n.div_ceil(args.client_threads);
        let started = Instant::now();
        let mut latencies = Latency::new();
        std::thread::scope(|scope| {
            let workers: Vec<_> = peers
                .chunks_mut(per_thread)
                .map(|mine| {
                    let stop = &stop;
                    scope.spawn(move || {
                        set_affinity(&[cpu]);
                        let mut latencies = Latency::new();
                        while !stop.load(Ordering::Relaxed) {
                            for peer in mine.iter_mut() {
                                let t = Instant::now();
                                peer.client.send_yo(&peer.to).unwrap();
                                latencies.record(t.elapsed());
                            }
                        }
                        latencies
                    })
                })
                .collect();
            std::thread::sleep(Duration::from_secs(args.seconds));
            stop.store(true, Ordering::Relaxed);
            for worker in workers {
                latencies.merge(worker.join().unwrap());
            }
        });
        let seconds = started.elapsed().as_secs_f64();
        let errors = http_errors.load(Ordering::Relaxed);
        println!(
            "one client core: {} Yos sent, {errors} HTTP errors, {:.0} Yos/s, send p50 {:.1} ms, p99 {:.1} ms",
            latencies.count,
            latencies.count as f64 / seconds,
            latencies.percentile(0.50),
            latencies.percentile(0.99),
        );
        println!(
            "first Yo: p50 {:.1} ms, p99 {:.1} ms",
            first_latency.percentile(0.50),
            first_latency.percentile(0.99)
        );
        assert_eq!(errors, 0, "the one-core send phase had HTTP errors");
        serde_json::json!({
            "cpu": cpu,
            "threads": n.div_ceil(per_thread),
            "seconds": seconds,
            "yos_sent": latencies.count,
            "yos_per_s": latencies.count as f64 / seconds,
            "send": latencies.json(),
            "first_yo": first_latency.json(),
        })
    });

    let hub_cpu = std::env::var("YO_HUB_CPU").ok().and_then(|cpu| cpu.parse::<usize>().ok());
    let result = serde_json::json!({
        "date": std::env::var("YO_DATE").unwrap_or_default(),
        "commit": std::env::var("YO_COMMIT").unwrap_or_default(),
        "cores": cores,
        "hub_cpu": hub_cpu,
        "client_cpu": client_cpu,
        "requested_seconds": args.seconds,
        "requested_raw_seconds": raw_seconds,
        "clients": n,
        "threads": args.threads,
        "registration": { "seconds": registration_s, "per_s": n as f64 / registration_s },
        "end_to_end": e2e,
        "hub_alone": raw,
        "one_client_core": one_core,
    });
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
    std::fs::create_dir_all(&args.out).unwrap();
    let name = format!("{}/{}-{}c.json", args.out, std::env::var("YO_DATE").unwrap_or_else(|_| "run".into()), n);
    std::fs::write(&name, serde_json::to_string_pretty(&result).unwrap() + "\n").unwrap();
    println!("written to {name}");
    hub.stop();
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(&states);
    for peer in &peers {
        let receiver = &peers[(peer.index + 1) % n];
        assert_eq!(peer.sent, receiver.received, "mismatched Yo count on ring hop {}", peer.index);
    }
    assert_eq!((dropped, waiting, e2e_errors, errors), (0, 0, 0, 0), "load left drops, pending messages or errors");
}
