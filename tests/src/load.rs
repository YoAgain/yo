//! yo-load: how many Yos the hub moves.
//!
//!   yo-load [--clients 200] [--seconds 15] [--threads 16] [--connections 64] [--out tests/results]
//!
//! Phase 1, end to end: real clients (MLS, HTTP, the hub, fetch, decrypt) in a ring, each sending Yos to the next
//! and receiving its own. Clients and hub share this machine, so this is the whole system on one box.
//! Phase 2, the hub alone: well-formed envelopes delivered over many connections at once, as fast as it takes them.
//! The hub runs in-process on a SQLite file (WAL), as in production. Results are printed and written as JSON.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use yo_core::{Client, Content, Event};
use yo_hub::config::{Config, Limits};
use yo_tests::{random_u64, register, username, Http, TestHub};

struct Args {
    clients: usize,
    seconds: u64,
    threads: usize,
    connections: usize,
    out: String,
}

fn args() -> Args {
    let mut args = Args { clients: 200, seconds: 15, threads: 16, connections: 64, out: "tests/results".into() };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let value = it.next().unwrap_or_default();
        match flag.as_str() {
            "--clients" => args.clients = value.parse().expect("--clients"),
            "--seconds" => args.seconds = value.parse().expect("--seconds"),
            "--threads" => args.threads = value.parse().expect("--threads"),
            "--connections" => args.connections = value.parse().expect("--connections"),
            "--out" => args.out = value,
            other => panic!("unknown flag {other}"),
        }
    }
    args
}

fn percentile(sorted: &[u64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i] as f64 / 1000.0
}

fn latency(mut micros: Vec<u64>) -> serde_json::Value {
    micros.sort_unstable();
    serde_json::json!({
        "p50_ms": percentile(&micros, 0.50),
        "p90_ms": percentile(&micros, 0.90),
        "p99_ms": percentile(&micros, 0.99),
        "max_ms": percentile(&micros, 1.0),
    })
}

fn yos(events: &[Event]) -> u64 {
    events.iter().filter(|e| matches!(e, Event::Message { content: Content::Yo, .. })).count() as u64
}

fn main() {
    let args = args();
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
        "yo-load: {} clients, {} s per phase, {} threads, {} connections, {cores} cores",
        args.clients, args.seconds, args.threads, args.connections
    );

    // ---- registration ----
    let base = random_u64() % 1_000_000_000;
    let started = Instant::now();
    let per_thread = args.clients.div_ceil(args.threads);
    let registering: Vec<_> = (0..args.threads)
        .map(|t| {
            let url = url.clone();
            std::thread::spawn(move || {
                (t * per_thread..((t + 1) * per_thread))
                    .map(|i| (i, register(Http::new(url.clone()), &username(base + i as u64), None)))
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let mut clients: Vec<(usize, Client)> = registering.into_iter().flat_map(|t| t.join().unwrap()).collect();
    clients.retain(|(i, _)| *i < args.clients);
    clients.sort_by_key(|(i, _)| *i);
    let registration_s = started.elapsed().as_secs_f64();
    println!(
        "registered {} in {registration_s:.1} s ({:.0}/s, each with 51 KeyPackages)",
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
        client.sync(0).unwrap();
    }
    println!("{n} chats set up in {:.1} s", started.elapsed().as_secs_f64());

    // ---- phase 1: end to end ----
    let stop = Arc::new(AtomicBool::new(false));
    let sent = Arc::new(AtomicU64::new(0));
    let received = Arc::new(AtomicU64::new(0));
    let send_latency = Arc::new(Mutex::new(Vec::new()));
    let mut groups: Vec<Vec<(usize, Client)>> = (0..args.threads).map(|_| Vec::new()).collect();
    for (k, entry) in clients.into_iter().enumerate() {
        groups[k % args.threads].push(entry);
    }
    let started = Instant::now();
    let workers: Vec<_> = groups
        .into_iter()
        .map(|mut mine| {
            let (stop, sent, received, send_latency) =
                (stop.clone(), sent.clone(), received.clone(), send_latency.clone());
            std::thread::spawn(move || {
                let mut latencies = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    for (i, client) in mine.iter_mut() {
                        let t = Instant::now();
                        let events = client.send_yo(&username(base + ((*i + 1) % n) as u64)).unwrap();
                        latencies.push(t.elapsed().as_micros() as u64);
                        sent.fetch_add(1, Ordering::Relaxed);
                        received.fetch_add(yos(&events), Ordering::Relaxed);
                    }
                }
                send_latency.lock().unwrap().extend(latencies);
                mine
            })
        })
        .collect();
    std::thread::sleep(Duration::from_secs(args.seconds));
    stop.store(true, Ordering::Relaxed);
    let mut clients: Vec<(usize, Client)> = workers.into_iter().flat_map(|w| w.join().unwrap()).collect();
    let e2e_s = started.elapsed().as_secs_f64();
    // What is still in flight is received now; it counts, the time does not.
    for (_, client) in clients.iter_mut() {
        received.fetch_add(yos(&client.sync(0).unwrap()), Ordering::Relaxed);
    }
    let dropped: u64 = clients.iter().map(|(_, c)| c.dropped()).sum();
    let (sent, received) = (sent.load(Ordering::Relaxed), received.load(Ordering::Relaxed));
    let e2e = serde_json::json!({
        "seconds": e2e_s,
        "yos_sent": sent,
        "yos_received": received,
        "dropped": dropped,
        "yos_per_s": sent as f64 / e2e_s,
        "send": latency(std::mem::take(&mut *send_latency.lock().unwrap())),
    });
    println!("end to end: {sent} Yos sent, {received} received, {dropped} dropped, {:.0} Yos/s", sent as f64 / e2e_s);

    // ---- phase 2: the hub alone ----
    // Each user with its device check, learned once as any sender would (spec 4.6).
    let targets: Vec<(String, String)> = clients
        .iter()
        .map(|(_, c)| {
            let hash = yo_tests::devices_hash(&url, c.user_id()).expect("a registered user");
            (yo_wire::api::deliver_path(&yo_tests::user_id(c.user_id())), yo_wire::b64(&hash))
        })
        .collect();
    let envelope = yo_tests::junk(0x5A);
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let raw = runtime.block_on(async {
        let client = reqwest::Client::builder().pool_max_idle_per_host(args.connections).build().unwrap();
        let deadline = Instant::now() + Duration::from_secs(args.seconds);
        let tasks: Vec<_> = (0..args.connections)
            .map(|c| {
                let (client, url, envelope, targets) = (client.clone(), url.clone(), envelope.clone(), targets.clone());
                tokio::spawn(async move {
                    let mut latencies = Vec::new();
                    let mut errors = 0u64;
                    let mut k = c;
                    while Instant::now() < deadline {
                        let (path, hash) = &targets[k % targets.len()];
                        k += 1;
                        let t = Instant::now();
                        let request = client
                            .post(format!("{url}{path}"))
                            .header(yo_wire::api::HEADER_DEVICES, hash.as_str())
                            .body(envelope.clone());
                        match request.send().await {
                            Ok(r) if r.status().as_u16() == 202 => latencies.push(t.elapsed().as_micros() as u64),
                            _ => errors += 1,
                        }
                    }
                    (latencies, errors)
                })
            })
            .collect();
        let started = Instant::now();
        let mut all = Vec::new();
        let mut errors = 0;
        for task in tasks {
            let (latencies, e) = task.await.unwrap();
            all.extend(latencies);
            errors += e;
        }
        (all, errors, started.elapsed().as_secs_f64())
    });
    let (latencies, errors, raw_s) = raw;
    let delivered = latencies.len() as u64;
    let raw = serde_json::json!({
        "connections": args.connections,
        "seconds": raw_s,
        "delivered": delivered,
        "errors": errors,
        "per_s": delivered as f64 / raw_s,
        "latency": latency(latencies),
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
    drop(hub);
    let _ = std::fs::remove_dir_all(dir);
}
