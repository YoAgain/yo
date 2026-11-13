//! Checks a running YoGPT: registers a throwaway username, sends Yos one after another and counts the answers.
//!
//!   cargo run -p yo-gpt --example ping -- [https://dev.justyo.app] [YOGPT] [rounds 1]
//!
//! The throwaway name (PING and digits) stays on the hub until it is released (spec 6.4).

use std::sync::Arc;
use std::time::{Duration, Instant};
use yo_core::{Client, Content, Event, Push, Response, Transport};

struct Http(reqwest::blocking::Client, String);

impl Transport for Http {
    fn request(&self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Result<Response, String> {
        let mut request =
            self.0.request(method.parse().map_err(|_| "method")?, format!("{}{path}", self.1)).body(body.to_vec());
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request.send().map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        Ok(Response { status, body: response.bytes().map_err(|e| e.to_string())?.to_vec() })
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let hub = args.next().unwrap_or_else(|| "https://dev.justyo.app".into());
    let bot = args.next().unwrap_or_else(|| "YOGPT".into());
    let rounds: usize = args.next().map_or(1, |n| n.parse().expect("rounds: a number"));
    let http = reqwest::blocking::Client::builder().timeout(Duration::from_secs(60)).build().unwrap();
    let mut n = [0u8; 4];
    getrandom::getrandom(&mut n).unwrap();
    let name = format!("PING{}", u32::from_le_bytes(n) % 100_000_000);
    let mut client =
        Client::register_username(Arc::new(Http(http, hub.clone())), &name, None, None, Push::none()).unwrap();
    println!("registered {name} on {hub}");
    let mut counts = [0usize; 6];
    for round in 1..=rounds {
        let sent = Instant::now();
        client.send_yo(&bot).unwrap();
        // Until 3 seconds without a Yo after the first, or 15 seconds without any.
        let (mut yos, mut first, mut last) = (0, None, sent);
        while last.elapsed() < Duration::from_secs(if yos == 0 { 15 } else { 3 }) {
            for event in client.sync(1).unwrap() {
                if let Event::Message { content: Content::Yo, .. } = event {
                    yos += 1;
                    first.get_or_insert(sent.elapsed());
                    last = Instant::now();
                }
            }
            client.acknowledge_events();
        }
        counts[yos.min(5)] += 1;
        let first = first.map_or("-".into(), |d| format!("{} ms", d.as_millis()));
        println!("round {round}: {yos} Yo back, the first after {first}");
    }
    println!("answers by number of Yos (0..=5): {counts:?}");
    if counts[0] > 0 {
        std::process::exit(1);
    }
}
