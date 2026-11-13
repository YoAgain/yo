//! YoGPT as it runs: its binary against a hub in-process, a client sending it Yos.

use std::process::{Child, Command};
use std::time::{Duration, Instant};
use yo_core::{Client, Content, Event};
use yo_tests::{phone, random_u64, register, Http, TestHub};

struct Bot(Child);

impl Bot {
    fn start(hub: &TestHub, state: &std::path::Path) -> Bot {
        let child = Command::new(env!("CARGO_BIN_EXE_yo-gpt"))
            .args(["--hub", &hub.url(), "--username", "YOGPT", "--state"])
            .arg(state)
            .spawn()
            .unwrap();
        Bot(child)
    }
}

impl Drop for Bot {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Yos from anyone until `quiet` passes without one.
fn yos_until_quiet(client: &mut Client, quiet: Duration) -> Vec<String> {
    let mut from = Vec::new();
    let mut last = Instant::now();
    while last.elapsed() < quiet {
        for event in client.sync(1).unwrap() {
            if let Event::Message { user, content: Content::Yo, .. } = event {
                from.push(user);
                last = Instant::now();
            }
        }
        client.acknowledge_events();
    }
    from
}

/// Sends a Yo to YOGPT (once it is there) and collects the answer; returns who answered.
fn round_trip(client: &mut Client) -> Vec<String> {
    let start = Instant::now();
    while client.send_yo("YOGPT").is_err() {
        assert!(start.elapsed() < Duration::from_secs(30), "YOGPT never came");
        std::thread::sleep(Duration::from_millis(200));
    }
    // It thinks up to 5 seconds, then up to 5 Yos at most a second apart.
    let from = yos_until_quiet(client, Duration::from_secs(8));
    assert!((1..=5).contains(&from.len()), "{} Yos back", from.len());
    from
}

#[test]
fn yogpt_answers_a_yo_and_again_after_a_restart() {
    let hub = TestHub::start();
    let state = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("yogpt-{}", random_u64()));
    let mut alice = register(Http::new(hub.url()), &phone(random_u64() % 1_000_000_000), None);

    let bot = Bot::start(&hub, &state);
    let first = round_trip(&mut alice);
    drop(bot);
    for file in ["account", "key", "state"] {
        let mode = std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(state.join(file)).unwrap().permissions());
        assert_eq!(mode & 0o777, 0o600, "{file}");
    }

    // The same account from the saved state: the answer comes from the same user.
    let _bot = Bot::start(&hub, &state);
    let second = round_trip(&mut alice);
    assert!(first.iter().chain(&second).all(|user| *user == first[0]));

    // Anything but a Yo gets nothing back.
    alice.send("YOGPT", Content::Text("rm -rf /".into())).unwrap();
    assert!(yos_until_quiet(&mut alice, Duration::from_secs(7)).is_empty());
    let _ = std::fs::remove_dir_all(&state);
}
