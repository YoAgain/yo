//! yo-send: an account that sends Yos, for timing the apps against a hub (app/ios/TESTING.md, "Speed").
//!
//!   yo-send --url https://dev.justyo.app --state FILE --name NAME --to NAME [--count 1] [--every-ms 0] [--sync]
//!
//! The account is registered as NAME the first time and kept in FILE (sealed under a fixed key: a test account).
//! `--sync` first receives (a new chat's Welcome, Yos sent to it). Each Yo is a line: when it was sent (wall clock,
//! ms, to compare with the app's YO_PERF marks) and how long `send_yo` took.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use yo_core::{Client, Push, Store};
use yo_tests::Http;

struct File(String);

impl Store for File {
    fn save(&self, blob: Vec<u8>) -> Result<(), String> {
        std::fs::write(&self.0, blob).map_err(|e| e.to_string())
    }
}

const KEY: [u8; 32] = [9; 32];

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut url, mut state, mut name, mut to) = (String::new(), String::new(), String::new(), String::new());
    let (mut count, mut every_ms, mut sync) = (1u32, 0u64, false);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--sync" => sync = true,
            "--url" => url = args.next().expect("--url"),
            "--state" => state = args.next().expect("--state"),
            "--name" => name = args.next().expect("--name"),
            "--to" => to = args.next().expect("--to"),
            "--count" => count = args.next().and_then(|v| v.parse().ok()).expect("--count"),
            "--every-ms" => every_ms = args.next().and_then(|v| v.parse().ok()).expect("--every-ms"),
            other => panic!("unknown flag {other}"),
        }
    }
    let http = Http::new(url);
    let mut client = match std::fs::read(&state) {
        Ok(blob) => Client::restore(http, &blob, &KEY).expect("restore"),
        Err(_) => Client::register_username(http, &name, None, None, Push::none()).expect("register"),
    };
    client.persist_to(Arc::new(File(state)), KEY).expect("state file");
    if sync {
        let events = client.sync(0).expect("sync");
        println!("synced: {} events", events.len());
    }
    let start = Instant::now();
    for i in 0..count {
        let at = Instant::now();
        let wall = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64() * 1000.0;
        let result = client.send_yo(&to);
        println!("yo {i} at {wall:.3} took {:.1} ms {}", at.elapsed().as_secs_f64() * 1000.0, result.is_ok());
        let next = start + Duration::from_millis(every_ms * u64::from(i + 1));
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
}
