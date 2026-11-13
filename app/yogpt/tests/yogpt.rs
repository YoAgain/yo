//! YoGPT as it runs: its binary against hubs in-process, clients sending it Yos.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use yo_core::media::{self, Media};
use yo_core::{Client, Content, Event, Push};
use yo_tests::{phone, random_u64, register, Http, TestHub};

struct Bot(Child);

impl Bot {
    /// One account per hub, each with its state directory.
    fn start(accounts: &[(&TestHub, &Path)]) -> Bot {
        Self::start_with(accounts, &[])
    }

    fn start_with(accounts: &[(&TestHub, &Path)], args: &[&str]) -> Bot {
        let mut command = Command::new(env!("CARGO_BIN_EXE_yo-gpt"));
        command.args(args);
        for (i, (hub, state)) in accounts.iter().enumerate() {
            command.arg("--account").arg(format!("hub{i}={},{}", hub.url(), state.display()));
        }
        Bot(command.spawn().unwrap())
    }
}

impl Drop for Bot {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn state_dir() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("yogpt-{}", random_u64()))
}

fn someone(hub: &TestHub) -> Client {
    register(Http::new(hub.url()), &phone(random_u64() % 1_000_000_000), None)
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
    // It thinks up to 3 seconds, then up to 5 Yos at most a second apart.
    let from = yos_until_quiet(client, Duration::from_secs(8));
    assert!((1..=5).contains(&from.len()), "{} Yos back", from.len());
    from
}

/// Takes YOGPT over on its hub with the bot's own account secret, as a new phone would: the bot's device is gone.
fn take_over(hub: &TestHub, state: &Path) {
    let secret: [u8; 32] = std::fs::read(state.join("account")).unwrap().try_into().unwrap();
    Client::register_username(Http::new(hub.url()), "YOGPT", Some(secret), None, Push::none()).unwrap();
}

#[test]
fn yogpt_answers_a_yo_and_again_after_a_restart() {
    let hub = TestHub::start();
    let state = state_dir();
    let mut alice = someone(&hub);

    let bot = Bot::start(&[(&hub, &state)]);
    let first = round_trip(&mut alice);
    drop(bot);
    for file in ["account", "key", "state"] {
        let mode = std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(state.join(file)).unwrap().permissions());
        assert_eq!(mode & 0o777, 0o600, "{file}");
    }

    // The same account from the saved state: the answer comes from the same user.
    let _bot = Bot::start(&[(&hub, &state)]);
    let second = round_trip(&mut alice);
    assert!(first.iter().chain(&second).all(|user| *user == first[0]));

    // Anything but a Yo gets nothing back.
    alice.send("YOGPT", Content::Text("rm -rf /".into())).unwrap();
    assert!(yos_until_quiet(&mut alice, Duration::from_secs(7)).is_empty());
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn two_accounts_in_one_process_stop_one_by_one() {
    let (a, b) = (TestHub::start(), TestHub::start());
    let (state_a, state_b) = (state_dir(), state_dir());
    let (mut alice, mut bob) = (someone(&a), someone(&b));
    let mut bot = Bot::start(&[(&a, &state_a), (&b, &state_b)]);
    round_trip(&mut alice);
    round_trip(&mut bob);

    // B's device is gone: B stops (after its long poll), A goes on, the process too.
    take_over(&b, &state_b);
    std::thread::sleep(Duration::from_secs(30));
    assert!(bot.0.try_wait().unwrap().is_none(), "the process ended with one account left");
    round_trip(&mut alice);

    // A's too: none left, the process ends with 1.
    take_over(&a, &state_a);
    let start = Instant::now();
    let status = loop {
        if let Some(status) = bot.0.try_wait().unwrap() {
            break status;
        }
        assert!(start.elapsed() < Duration::from_secs(40), "still running without accounts");
        std::thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(status.code(), Some(1));
    let _ = std::fs::remove_dir_all(&state_a);
    let _ = std::fs::remove_dir_all(&state_b);
}

/// Yos and pictures from anyone until `quiet` passes without one: how many Yos, and the pictures, opened.
fn answers_until_quiet(client: &mut Client, quiet: Duration) -> (usize, Vec<Vec<u8>>) {
    let (mut yos, mut pictures) = (0, Vec::<Media>::new());
    let mut last = Instant::now();
    while last.elapsed() < quiet {
        for event in client.sync(1).unwrap() {
            match event {
                Event::Message { content: Content::Yo, .. } => yos += 1,
                Event::Message { content: Content::Photo(media), .. } => pictures.push(media),
                _ => continue,
            }
            last = Instant::now();
        }
        client.acknowledge_events();
    }
    let transport = client.transport();
    let opened = pictures.iter().map(|m| media::fetch_open(transport.as_ref(), m).unwrap().unwrap()).collect();
    (yos, opened)
}

/// A JPEG's start, 720 by 1280, and noise: what YoGPT sends is these bytes as they are.
fn reply_picture() -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xC0, 0, 17, 8, 0x05, 0x00, 0x02, 0xD0, 3];
    bytes.extend((0..5000).map(|_| random_u64() as u8));
    bytes
}

#[test]
fn yogpt_answers_a_photo_with_a_picture_and_its_yos_carry_one() {
    let hub = TestHub::start();
    let state = state_dir();
    let replies = state_dir();
    std::fs::create_dir_all(&replies).unwrap();
    let files: Vec<Vec<u8>> = (1..=3).map(|_| reply_picture()).collect();
    for (i, bytes) in files.iter().enumerate() {
        std::fs::write(replies.join(format!("{:03}.jpg", i + 1)), bytes).unwrap();
    }
    std::fs::write(replies.join("README.md"), b"not a picture").unwrap();
    let mut alice = someone(&hub);
    // Every answer with a picture: one of its Yos carries one, from the directory, as it is there.
    let replies_arg = replies.to_str().unwrap();
    let _bot = Bot::start_with(&[(&hub, &state)], &["--replies", replies_arg, "--picture-every", "1"]);
    let start = Instant::now();
    while alice.send_yo("YOGPT").is_err() {
        assert!(start.elapsed() < Duration::from_secs(30), "YOGPT never came");
        std::thread::sleep(Duration::from_millis(200));
    }
    let (yos, pictures) = answers_until_quiet(&mut alice, Duration::from_secs(8));
    assert_eq!(pictures.len(), 1, "{yos} Yos and {} pictures", pictures.len());
    assert!((0..=4).contains(&yos));
    assert!(files.contains(&pictures[0]));

    // A photo to it: one picture back, and the photo fetched into nothing (the hub has it no more).
    let (photo, sealed) = media::seal(b"a photo YoGPT never sees", 4, 3).unwrap();
    let fetches = alice.media_fetches("YOGPT").unwrap();
    assert_eq!(fetches, 1);
    alice.upload(&photo, fetches).send(&sealed).unwrap();
    alice.send("YOGPT", Content::Photo(photo.clone())).unwrap();
    let (yos, pictures) = answers_until_quiet(&mut alice, Duration::from_secs(6));
    assert_eq!((yos, pictures.len()), (0, 1));
    assert!(files.contains(&pictures[0]));
    assert_eq!(media::fetch(alice.transport().as_ref(), &photo).unwrap(), None, "drained");
    let _ = std::fs::remove_dir_all(&state);
    let _ = std::fs::remove_dir_all(&replies);
}

/// A burst of photos: their pictures come back over HTTP right after the thinking, each fetched as it comes; a picture
/// adds no seconds of its own (the sender's rhythm is its photos' spacing, here none).
#[test]
fn a_burst_of_photos_is_answered_with_its_pictures_at_once() {
    let hub = TestHub::start();
    let state = state_dir();
    let replies = state_dir();
    std::fs::create_dir_all(&replies).unwrap();
    // As large as a reply picture may be.
    let mut picture = reply_picture();
    picture.resize(500_000, 7);
    std::fs::write(replies.join("001.jpg"), &picture).unwrap();
    let mut alice = someone(&hub);
    let _bot = Bot::start_with(&[(&hub, &state)], &["--replies", replies.to_str().unwrap(), "--picture-every", "0"]);
    let start = Instant::now();
    while alice.send_yo("YOGPT").is_err() {
        assert!(start.elapsed() < Duration::from_secs(30), "YOGPT never came");
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = yos_until_quiet(&mut alice, Duration::from_secs(5));
    const N: usize = 10;
    let sent = Instant::now();
    for _ in 0..N {
        let (photo, sealed) = media::seal(b"a photo YoGPT never sees", 4, 3).unwrap();
        alice.upload(&photo, 1).send(&sealed).unwrap();
        alice.send("YOGPT", Content::Photo(photo)).unwrap();
    }
    let mut arrived = Vec::new();
    while arrived.len() < N && sent.elapsed() < Duration::from_secs(30) {
        for event in alice.sync(1).unwrap() {
            if let Event::Message { content: Content::Photo(m), .. } = event {
                assert_eq!(media::fetch_open(alice.transport().as_ref(), &m).unwrap().unwrap(), picture);
                arrived.push(sent.elapsed());
            }
        }
        alice.acknowledge_events();
    }
    assert_eq!(arrived.len(), N, "{arrived:?}");
    // Thinking takes up to 3 s; the ten pictures after it, sealed, uploaded, sent and fetched, well within 2 s more.
    assert!(arrived[N - 1] < Duration::from_secs(5), "{arrived:?}");
    assert!(arrived[N - 1] - arrived[0] < Duration::from_secs(2), "{arrived:?}");
    let _ = std::fs::remove_dir_all(&state);
    let _ = std::fs::remove_dir_all(&replies);
}
