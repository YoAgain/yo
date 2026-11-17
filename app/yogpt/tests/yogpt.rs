//! YoGPT as it runs: its binary against hubs in-process, clients sending it Yos.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use yo_core::media::{self, Media};
use yo_core::{Client, Content, Event, Push};
use yo_tests::{random_u64, register, username, Http, TestHub};

struct Bot(Child);

impl Bot {
    /// One account per hub, each with its state directory.
    fn start(accounts: &[(&TestHub, &Path)]) -> Bot {
        Self::start_with(accounts, &[])
    }

    fn start_with(accounts: &[(&TestHub, &Path)], args: &[&str]) -> Bot {
        Self::start_env(accounts, args, &[])
    }

    fn start_env(accounts: &[(&TestHub, &Path)], args: &[&str], env: &[(&str, &str)]) -> Bot {
        let mut command = Command::new(env!("CARGO_BIN_EXE_yo-gpt"));
        command.args(args).envs(env.iter().copied());
        for (i, (hub, state)) in accounts.iter().enumerate() {
            command.arg("--account").arg(format!("hub{i}={},{}", hub.url(), state.display()));
            command.arg("--trust-root").arg(format!("hub{i}={}", yo_wire::b64(&yo_tests::trust_root())));
            command.arg("--zk-params").arg(format!("hub{i}={}", yo_wire::b64(&yo_tests::zk_params())));
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
    register(Http::new(hub.url()), &username(random_u64() % 1_000_000_000), None)
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
fn two_accounts_survive_a_hub_connection_refusal() {
    let state = state_dir();
    std::fs::create_dir_all(&state).unwrap();
    let mut config = yo_hub::config::Config::for_tests();
    config.db_path = state.join("hub.sqlite").to_str().unwrap().to_owned();
    let hub = TestHub::start_with(config.clone(), "127.0.0.1:0");
    let other = TestHub::start();
    let other_state = state_dir();
    let mut alice = someone(&hub);
    let mut bob = someone(&other);
    let replies = state.join("replies");
    std::fs::create_dir_all(&replies).unwrap();
    std::fs::write(replies.join("001.jpg"), reply_picture()).unwrap();
    let mut bot = Bot::start_with(
        &[(&hub, &state.join("bot")), (&other, &other_state)],
        &["--replies", replies.to_str().unwrap()],
    );
    let first = round_trip(&mut alice);
    // Interrupt a sender's picture-answer queue too, rather than only an idle long poll.
    let (photo, sealed) = media::seal(b"not opened", 4, 3).unwrap();
    alice.upload(&photo, 1).unwrap().send(&sealed).unwrap();
    for _ in 0..10 {
        alice.send("YOGPT", Content::Photo(photo.clone())).unwrap();
    }
    let addr = hub.addr.to_string();
    hub.stop();
    // Refused connections last long enough for the receiver to fail and retry.
    std::thread::sleep(Duration::from_secs(4));
    round_trip(&mut bob);
    let _hub = TestHub::start_with(config, &addr);
    assert!(bot.0.try_wait().unwrap().is_none());
    let second = round_trip(&mut alice);
    assert!(second.iter().all(|user| *user == first[0]));
    drop(bot);
    let _ = std::fs::remove_dir_all(state);
    let _ = std::fs::remove_dir_all(other_state);
}

/// Runs the same bot process through five hub restarts, with traffic before and during each outage.
#[test]
#[ignore = "ten-minute hub restart soak"]
fn yogpt_keeps_answering_through_repeated_hub_restarts() {
    let state = state_dir();
    std::fs::create_dir_all(&state).unwrap();
    let mut config = yo_hub::config::Config::for_tests();
    config.db_path = state.join("hub.sqlite").to_str().unwrap().to_owned();
    let mut hub = TestHub::start_with(config.clone(), "127.0.0.1:0");
    let mut alice = someone(&hub);
    let mut bot = Bot::start(&[(&hub, &state.join("bot"))]);
    let yogpt = round_trip(&mut alice).remove(0);
    let mut rounds = 1;
    for cycle in 1..=5 {
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            assert!(round_trip(&mut alice).iter().all(|user| *user == yogpt));
            rounds += 1;
        }
        assert!(bot.0.try_wait().unwrap().is_none());
        let addr = hub.addr.to_string();
        hub.stop();
        alice.send_yo(&yogpt).unwrap();
        assert!(alice.waiting() > 0, "outage {cycle}: send was not queued");
        std::thread::sleep(Duration::from_secs(5));
        hub = TestHub::start_with(config.clone(), &addr);

        let answers = yos_until_quiet(&mut alice, Duration::from_secs(20));
        assert!((1..=5).contains(&answers.len()), "outage {cycle}: got {} replies", answers.len());
        assert!(answers.iter().all(|user| *user == yogpt));
        assert_eq!(alice.waiting(), 0);
        assert_eq!(alice.dropped(), 0);
        assert!(bot.0.try_wait().unwrap().is_none());
        rounds += 1;
        println!("hub restart {cycle}: queued Yo answered by the same bot");
    }
    assert!(hub.hub.audit().unwrap().is_empty());
    println!("{rounds} YoGPT round trips across five hub restarts");
    drop(bot);
    hub.stop();
    std::fs::remove_dir_all(state).unwrap();
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
    reply_picture_of(5000)
}

/// A start, a frame header (720 × 1280), a scan of `len` bytes (no 0xFF in them), the end: what YoGPT takes as a JPEG
/// and sends as it is.
fn reply_picture_of(len: usize) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xC0, 0, 17, 8, 0x05, 0x00, 0x02, 0xD0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    bytes.extend_from_slice(&[0xFF, 0xDA, 0, 2]);
    bytes.extend((0..len).map(|_| random_u64() as u8 & 0x7F));
    bytes.extend_from_slice(&[0xFF, 0xD9]);
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

    // A photo to it: one picture back. Without an acknowledgement, its blob stays until expiry.
    let (photo, sealed) = media::seal(b"a photo YoGPT never sees", 4, 3).unwrap();
    let fetches = alice.media_fetches("YOGPT").unwrap();
    assert_eq!(fetches, 1);
    alice.upload(&photo, fetches).unwrap().send(&sealed).unwrap();
    alice.send("YOGPT", Content::Photo(photo.clone())).unwrap();
    let (yos, pictures) = answers_until_quiet(&mut alice, Duration::from_secs(6));
    assert_eq!((yos, pictures.len()), (0, 1));
    assert!(files.contains(&pictures[0]));
    assert!(media::fetch(alice.transport().as_ref(), &photo).unwrap().is_some(), "unacknowledged blob stays");
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
    let picture = reply_picture_of(500_000);
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
        alice.upload(&photo, 1).unwrap().send(&sealed).unwrap();
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
    // Thinking takes up to 3 s; the ten pictures after it, sealed, uploaded, sent and fetched, together: a few seconds
    // more on a slow machine (CI), never one after the other at the pace of a Yo each.
    assert!(arrived[N - 1] < Duration::from_secs(10), "{arrived:?}");
    assert!(arrived[N - 1] - arrived[0] < Duration::from_secs(5), "{arrived:?}");
    let _ = std::fs::remove_dir_all(&state);
    let _ = std::fs::remove_dir_all(&replies);
}

/// Group Yos and photos it is sent, from the group (`group`) and from `from`, until `quiet` passes without one:
/// (Yos, photos).
fn group_answers(client: &mut Client, group: &str, from: &str, quiet: Duration) -> (usize, usize) {
    let (mut yos, mut photos, mut last) = (0, 0, Instant::now());
    while last.elapsed() < quiet {
        for event in client.sync(1).unwrap() {
            if let Event::GroupMessage { group: g, user, content, .. } = event {
                if g == group && user == from {
                    match content {
                        Content::Photo(_) => photos += 1,
                        _ => yos += 1,
                    }
                    last = Instant::now();
                }
            }
        }
        client.acknowledge_events();
    }
    (yos, photos)
}

/// In a group of three, YoGPT answers a Yo into the group, and a photo with a picture.
#[test]
fn yogpt_answers_in_a_group() {
    let hub = TestHub::start();
    let (state, replies) = (state_dir(), state_dir());
    std::fs::create_dir_all(&replies).unwrap();
    std::fs::write(replies.join("001.jpg"), reply_picture()).unwrap();
    // The group cooldown (30 s) short here: a Yo, then a photo a few seconds later, both answered.
    let env = [("YO_GPT_GROUP_COOLDOWN_MS", "1000")];
    let _bot = Bot::start_env(&[(&hub, &state)], &["--replies", replies.to_str().unwrap()], &env);
    let (mut alice, bob) = (someone(&hub), someone(&hub));
    let yogpt = round_trip(&mut alice).remove(0);
    let (group, _) = alice.create_group("Trio", &[yogpt.clone(), bob.user_id().to_string()]).unwrap();

    alice.send_group(&group, Content::Yo).unwrap();
    let (yos, photos) = group_answers(&mut alice, &group, &yogpt, Duration::from_secs(8));
    assert!((1..=5).contains(&yos) && photos == 0, "{yos} Yos, {photos} pictures into the group");

    let (photo, sealed) = media::seal(b"a group photo", 4, 3).unwrap();
    let fetches = alice.group_media_fetches(&group).unwrap();
    alice.upload(&photo, fetches).unwrap().send(&sealed).unwrap();
    alice.send_group(&group, Content::Photo(photo)).unwrap();
    let (yos, photos) = group_answers(&mut alice, &group, &yogpt, Duration::from_secs(8));
    assert_eq!((yos, photos), (0, 1), "a picture into the group");
    let _ = std::fs::remove_dir_all(&state);
    let _ = std::fs::remove_dir_all(&replies);
}

/// `--quiet-for`: a listed user (named in another case) gets nothing back, one to one or in a group, while another
/// does; and a group is answered once per cooldown, the Yos within it dropped.
#[test]
fn yogpt_stays_quiet_for_listed_users_and_answers_a_group_once_per_cooldown() {
    let hub = TestHub::start();
    let state = state_dir();
    let (mut leo, mut mia) = (someone(&hub), someone(&hub));
    let quiet = format!("{},nobody{}", leo.username().unwrap().to_lowercase(), random_u64() % 1000);
    let _bot = Bot::start_with(&[(&hub, &state)], &["--quiet-for", &quiet]);
    let yogpt = round_trip(&mut mia).remove(0);

    leo.send_yo("YOGPT").unwrap();
    assert!(yos_until_quiet(&mut leo, Duration::from_secs(7)).is_empty(), "a quiet-for sender was answered");

    let (group, _) = mia.create_group("Bots", &[yogpt.clone(), leo.user_id().to_string()]).unwrap();
    let _ = leo.sync(0).unwrap();
    leo.acknowledge_events();
    leo.send_group(&group, Content::Yo).unwrap();
    let (yos, _) = group_answers(&mut mia, &group, &yogpt, Duration::from_secs(7));
    assert_eq!(yos, 0, "answered a quiet-for sender in the group");

    // Mia is answered in the group; a second Yo of hers within 30 s is not.
    mia.send_group(&group, Content::Yo).unwrap();
    let (yos, _) = group_answers(&mut mia, &group, &yogpt, Duration::from_secs(7));
    assert!((1..=5).contains(&yos), "{yos} Yos into the group");
    mia.send_group(&group, Content::Yo).unwrap();
    let (yos, _) = group_answers(&mut mia, &group, &yogpt, Duration::from_secs(7));
    assert_eq!(yos, 0, "answered the group again within its cooldown");
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn echo_is_immediate_single_plain_bounded_and_logs_no_sender() {
    let hub = TestHub::start();
    let state = state_dir();
    std::fs::create_dir_all(&state).unwrap();
    let log_path = state.join("test.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_yo-gpt"));
    command.args(["--account", &format!("echo={},{},ECHO", hub.url(), state.display())]);
    command.args(["--echo", "echo", "--picture-every", "1", "--replies", "app/yogpt/replies"]);
    command.args(["--trust-root", &format!("echo={}", yo_wire::b64(&yo_tests::trust_root()))]);
    // A think of 8–9 s: a slow CI runner's echo stays far below it, so the bound below can't flake.
    command.env("YO_GPT_THINK_MS", "8000,9000");
    command.stderr(log);
    let bot = Bot(command.spawn().unwrap());
    let ready = Instant::now();
    while Client::username_free(Http::new(hub.url()).as_ref(), "ECHO").unwrap() {
        assert!(ready.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut alice = someone(&hub);
    let began = Instant::now();
    alice.send_yo("ECHO").unwrap();
    let mut answers = Vec::new();
    while answers.is_empty() {
        answers.extend(alice.sync(0).unwrap().into_iter().filter_map(|event| match event {
            Event::Message { content, .. } => Some(content),
            _ => None,
        }));
        assert!(began.elapsed() < Duration::from_millis(6000), "echo used the think path");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(answers, [Content::Yo]);
    assert!(yos_until_quiet(&mut alice, Duration::from_millis(1100)).is_empty(), "one answer only");
    // Media is not answered, even when the normal bot would always send a picture.
    let (photo, _) = media::seal(b"untrusted bytes", 1, 1).unwrap();
    alice.send("ECHO", Content::Photo(photo.clone())).unwrap();
    alice.send("ECHO", Content::Video { media: photo, duration_ms: 1 }).unwrap();
    assert!(yos_until_quiet(&mut alice, Duration::from_millis(1100)).is_empty());
    drop(bot);
    let log = std::fs::read_to_string(log_path).unwrap();
    assert!(!log.contains(alice.user_id()) && !log.contains(&alice.user_id()[..8]), "{log}");
    assert!(log.contains("echo answered=1"), "{log}");
    std::fs::remove_dir_all(state).unwrap();
}

#[test]
fn ping_answers_normal_yos_and_token_probes_independently() {
    let hub = TestHub::start();
    let state = state_dir();
    let mut command = Command::new(env!("CARGO_BIN_EXE_yo-gpt"));
    command.args(["--account", &format!("ping={},{},PING", hub.url(), state.display()), "--echo", "ping"]);
    command.args(["--trust-root", &format!("ping={}", yo_wire::b64(&yo_tests::trust_root()))]);
    let bot = Bot(command.spawn().unwrap());
    let ready = Instant::now();
    while Client::username_free(Http::new(hub.url()).as_ref(), "PING").unwrap() {
        assert!(ready.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut alice = someone(&hub);
    alice.send_yo("PING").unwrap();
    assert_eq!(yos_until_quiet(&mut alice, Duration::from_millis(300)).len(), 1);
    assert_eq!(alice.contacts().len(), 1);
    let began = Instant::now();
    assert!(alice.round_trip_probe("PING", 900).unwrap().is_some());
    assert!(began.elapsed() < Duration::from_millis(900), "probe used the think path");
    assert!(alice.sync(0).unwrap().is_empty(), "probe is silent for an ordinary PING contact too");
    alice.send_yo("PING").unwrap();
    assert_eq!(yos_until_quiet(&mut alice, Duration::from_millis(300)).len(), 1);
    drop(bot);
    std::fs::remove_dir_all(state).unwrap();
}
