//! The bots of `skills/` as they run on the server: each a script driving `yo`, against hubs in-process, people
//! sending them Yos as core clients.

mod common;

use common::*;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use yo_core::media;
use yo_core::{Client, Content, Event, Push};

#[test]
fn the_skills_rules_hold_without_a_hub() {
    let skills = concat!(env!("CARGO_MANIFEST_DIR"), "/../../skills");
    let status = Command::new("python3")
        .args(["-B", "-m", "unittest", "discover", "-s", skills, "-p", "test_*.py"])
        .status()
        .unwrap();
    assert!(status.success());
}

/// Sends a Yo to `name` (once it is there) and collects the answer; returns who answered.
fn round_trip(client: &mut Client, name: &str) -> Vec<String> {
    let start = Instant::now();
    while client.send_yo(name).is_err() {
        assert!(start.elapsed() < Duration::from_secs(30), "{name} never came");
        std::thread::sleep(Duration::from_millis(200));
    }
    // It thinks up to 3 seconds, then up to 5 Yos at most a second apart.
    let from = yos_until_quiet(client, Duration::from_secs(8));
    assert!((1..=5).contains(&from.len()), "{} Yos back", from.len());
    from
}

fn yogpt(yo: &Yo, args: &[&str], env: &[(&str, &str)]) -> Process {
    yo.skill("yogpt", "YOGPT", args, env)
}

fn replies(pictures: &[Vec<u8>]) -> std::path::PathBuf {
    let dir = state_dir();
    std::fs::create_dir_all(&dir).unwrap();
    for (i, bytes) in pictures.iter().enumerate() {
        std::fs::write(dir.join(format!("{:03}.jpg", i + 1)), bytes).unwrap();
    }
    std::fs::write(dir.join("README.md"), b"not a picture").unwrap();
    dir
}

#[test]
fn yogpt_answers_a_yo_and_again_after_a_restart() {
    let hub = TestHub::start();
    let yo = Yo::new(&hub);
    let mut alice = someone(&hub);
    let bot = yogpt(&yo, &[], &[]);
    let first = round_trip(&mut alice, "YOGPT");
    drop(bot);
    // The same account from the saved state: the answer comes from the same user.
    let _bot = yogpt(&yo, &[], &[]);
    let second = round_trip(&mut alice, "YOGPT");
    assert!(first.iter().chain(&second).all(|user| *user == first[0]));
    let _ = std::fs::remove_dir_all(&yo.state);
}

/// The hub away for a while (connections refused) with photos owed: YoGPT goes on once it is back.
#[test]
fn yogpt_survives_a_hub_connection_refusal() {
    let dir = state_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let mut config = yo_hub::config::Config::for_tests();
    config.db_path = dir.join("hub.sqlite").to_str().unwrap().to_owned();
    let hub = TestHub::start_with(config.clone(), "127.0.0.1:0");
    let yo = Yo::new(&hub);
    let mut alice = someone(&hub);
    let replies = replies(&[picture_of(5000)]);
    let mut bot = yogpt(&yo, &["--replies", replies.to_str().unwrap()], &[]);
    let first = round_trip(&mut alice, "YOGPT");
    let (photo, sealed) = media::seal(b"not opened", 4, 3).unwrap();
    alice.upload(&photo, 1).unwrap().send(&sealed).unwrap();
    for _ in 0..10 {
        alice.send("YOGPT", Content::Photo(photo.clone())).unwrap();
    }
    let addr = hub.addr.to_string();
    hub.stop();
    std::thread::sleep(Duration::from_secs(4));
    let _hub = TestHub::start_with(config, &addr);
    assert!(bot.running());
    let _ = yos_until_quiet(&mut alice, Duration::from_secs(8));
    let second = round_trip(&mut alice, "YOGPT");
    assert!(second.iter().all(|user| *user == first[0]));
    drop(bot);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(replies);
    let _ = std::fs::remove_dir_all(&yo.state);
}

/// A directory with the account's secret but no state (a takeover cut short): the skill stays stopped (4) and never
/// registers the account anew, so the device the hub knows keeps its place and its inbox.
#[test]
fn a_skill_without_a_registered_account_stays_stopped() {
    let hub = TestHub::start();
    let (mut bot, mut pal) = (someone(&hub), someone(&hub));
    let yo = Yo::new(&hub);
    std::fs::create_dir_all(&yo.state).unwrap();
    std::fs::write(yo.state.join("account"), bot.account_secret()).unwrap();
    std::fs::write(yo.state.join("key"), [9u8; 32]).unwrap();
    pal.send_yo(bot.user_id()).unwrap();
    for name in ["yogpt", "ping", "pingtest"] {
        let mut skill = Process::spawn(yo.skill_command(name, &[], &[]));
        assert_eq!(skill.exit_within(Duration::from_secs(20)), Some(4), "{name}");
    }
    assert_eq!(yos_until_quiet(&mut bot, Duration::from_millis(500)).len(), 1, "the device and its inbox stay");
    let _ = std::fs::remove_dir_all(&yo.state);
}

/// Its device taken over by a new phone of the account: the skill ends with 3 (systemd leaves it stopped).
#[test]
fn yogpt_stops_when_the_hub_no_longer_knows_its_device() {
    let hub = TestHub::start();
    let yo = Yo::new(&hub);
    let mut alice = someone(&hub);
    let mut bot = yogpt(&yo, &[], &[]);
    round_trip(&mut alice, "YOGPT");
    let secret: [u8; 32] = std::fs::read(yo.state.join("account")).unwrap().try_into().unwrap();
    Client::register_username(Http::new(hub.url()), "YOGPT", Some(secret), None, Push::none()).unwrap();
    assert_eq!(bot.exit_within(Duration::from_secs(40)), Some(3));
    let _ = std::fs::remove_dir_all(&yo.state);
}

#[test]
fn yogpt_answers_a_photo_with_a_picture_and_its_yos_carry_one() {
    let hub = TestHub::start();
    let yo = Yo::new(&hub);
    let files: Vec<Vec<u8>> = (1..=3).map(|_| picture_of(5000)).collect();
    let replies = replies(&files);
    let mut alice = someone(&hub);
    // Every answer with a picture: one of its Yos carries one, from the directory, as it is there.
    let _bot = yogpt(&yo, &["--replies", replies.to_str().unwrap(), "--picture-every", "1"], &[]);
    let start = Instant::now();
    while alice.send_yo("YOGPT").is_err() {
        assert!(start.elapsed() < Duration::from_secs(30), "YOGPT never came");
        std::thread::sleep(Duration::from_millis(200));
    }
    let (yos, pictures) = answers_until_quiet(&mut alice, Duration::from_secs(8));
    assert_eq!(pictures.len(), 1, "{yos} Yos and {} pictures", pictures.len());
    assert!((0..=4).contains(&yos));
    assert!(files.contains(&pictures[0]));

    // A photo to it: one picture back. Its blob, fetched into nothing, is acknowledged once: of two fetches one is left.
    let (photo, sealed) = media::seal(b"a photo YoGPT never sees", 4, 3).unwrap();
    assert_eq!(alice.media_fetches("YOGPT").unwrap(), 1);
    alice.upload(&photo, 2).unwrap().send(&sealed).unwrap();
    alice.send("YOGPT", Content::Photo(photo.clone())).unwrap();
    let (yos, pictures) = answers_until_quiet(&mut alice, Duration::from_secs(6));
    assert_eq!((yos, pictures.len()), (0, 1));
    assert!(files.contains(&pictures[0]));
    let transport = alice.transport();
    assert!(media::fetch(transport.as_ref(), &photo).unwrap().is_some(), "one fetch left");
    media::ack(transport.as_ref(), &photo.id).unwrap();
    assert!(media::fetch(transport.as_ref(), &photo).unwrap().is_none(), "the last fetch taken: gone");
    let _ = std::fs::remove_dir_all(&yo.state);
    let _ = std::fs::remove_dir_all(&replies);
}

/// A burst of photos: their pictures come back right after the thinking; a picture adds no seconds of its own.
#[test]
fn a_burst_of_photos_is_answered_with_its_pictures_at_once() {
    let hub = TestHub::start();
    let yo = Yo::new(&hub);
    // As large as a reply picture may be.
    let picture = picture_of(500_000);
    let replies = replies(std::slice::from_ref(&picture));
    let mut alice = someone(&hub);
    let _bot = yogpt(&yo, &["--replies", replies.to_str().unwrap()], &[]);
    round_trip(&mut alice, "YOGPT");
    const N: usize = 10;
    let sent = Instant::now();
    for _ in 0..N {
        let (photo, sealed) = media::seal(b"a photo YoGPT never sees", 4, 3).unwrap();
        alice.upload(&photo, 1).unwrap().send(&sealed).unwrap();
        alice.send("YOGPT", Content::Photo(photo)).unwrap();
    }
    // The answers keep the rhythm the photos came in: however long sending them took here.
    let sending = sent.elapsed();
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
    // Thinking takes up to 3 s; the ten pictures after it in the photos' rhythm, a few seconds more on a loaded machine,
    // never one after the other at the pace of a thinking each.
    assert!(arrived[N - 1] < sending + Duration::from_secs(12), "{sending:?} {arrived:?}");
    assert!(arrived[N - 1] - arrived[0] < sending + Duration::from_secs(5), "{sending:?} {arrived:?}");
    let _ = std::fs::remove_dir_all(&yo.state);
    let _ = std::fs::remove_dir_all(&replies);
}

/// Group Yos and photos from the group (`group`) and from `from`, until `quiet` passes without one: (Yos, photos).
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

/// In a group of three, YoGPT answers a Yo into the group, and a photo with a picture; left alone in it, it leaves.
#[test]
fn yogpt_answers_in_a_group_and_leaves_it_when_alone() {
    let hub = TestHub::start();
    let yo = Yo::new(&hub);
    let replies = replies(&[picture_of(5000)]);
    // The group cooldown (30 s) short here: a Yo, then a photo a few seconds later, both answered.
    let _bot = yogpt(&yo, &["--replies", replies.to_str().unwrap()], &[("YOGPT_GROUP_COOLDOWN_MS", "1000")]);
    let (mut alice, mut bob) = (someone(&hub), someone(&hub));
    let yogpt = round_trip(&mut alice, "YOGPT").remove(0);
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

    let _ = bob.sync(0).unwrap();
    bob.leave_group(&group).unwrap();
    alice.leave_group(&group).unwrap();
    let start = Instant::now();
    loop {
        let groups = yo.ok(&["groups"]);
        if groups.as_array().unwrap().iter().all(|g| g["left"] == true) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(40), "still in the empty group: {groups}");
        std::thread::sleep(Duration::from_millis(300));
    }
    let _ = std::fs::remove_dir_all(&yo.state);
    let _ = std::fs::remove_dir_all(&replies);
}

/// `--quiet-for`: a listed user (named in another case) gets nothing back, one to one or in a group, while another
/// does; and a group is answered once per cooldown, the Yos within it dropped.
#[test]
fn yogpt_stays_quiet_for_listed_users_and_answers_a_group_once_per_cooldown() {
    let hub = TestHub::start();
    let yo = Yo::new(&hub);
    let (mut leo, mut mia) = (someone(&hub), someone(&hub));
    let quiet = format!("{},nobody{}", leo.username().unwrap().to_lowercase(), random_u64() % 1000);
    let _bot = yogpt(&yo, &["--quiet-for", &quiet], &[]);
    let yogpt = round_trip(&mut mia, "YOGPT").remove(0);

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
    let _ = std::fs::remove_dir_all(&yo.state);
}

/// The operator's mailbox: a picture put into `outbox` goes to the name, a bad one is renamed `.failed`; `stats.json`
/// tells how many people YoGPT knows.
#[test]
fn yogpt_mails_the_operators_pictures_and_writes_its_stats() {
    let hub = TestHub::start();
    let yo = Yo::new(&hub);
    let mut alice = someone(&hub);
    let _bot = yogpt(&yo, &[], &[]);
    round_trip(&mut alice, "YOGPT");
    let picture = picture_of(4000);
    let outbox = yo.state.join("outbox");
    std::fs::write(outbox.join(format!("{}.hello.jpg", alice.username().unwrap())), &picture).unwrap();
    std::fs::write(outbox.join("nobody.bad.jpg"), b"not a jpeg").unwrap();
    let start = Instant::now();
    let mut pictures = Vec::new();
    while pictures.is_empty() {
        assert!(start.elapsed() < Duration::from_secs(30), "no mail came");
        pictures = answers_until_quiet(&mut alice, Duration::from_secs(1)).1;
    }
    assert_eq!(pictures, [picture]);
    assert!(outbox.join("nobody.bad.failed").exists());
    let stats: serde_json::Value =
        serde_json::from_slice(&std::fs::read(yo.state.join("stats.json")).unwrap()).unwrap();
    assert_eq!(stats["contacts"], 1);
    let _ = std::fs::remove_dir_all(&yo.state);
}

fn ping(yo: &Yo, args: &[&str], env: &[(&str, &str)]) -> Process {
    yo.skill("ping", "PING", args, env)
}

#[test]
fn echo_is_immediate_single_plain_bounded_and_logs_no_sender() {
    let hub = TestHub::start();
    let yo = Yo::new(&hub);
    std::fs::create_dir_all(&yo.state).unwrap();
    let log_path = yo.state.join("test.log");
    yo.registered_as("ECHO");
    let mut command = yo.skill_command("ping", &[], &[]);
    command.stderr(std::fs::File::create(&log_path).unwrap()).stdout(Stdio::null());
    let bot = Process::spawn(command);
    let mut alice = someone(&hub);
    let began = Instant::now();
    alice.send_yo("ECHO").unwrap();
    let mut answers = Vec::new();
    while answers.is_empty() {
        answers.extend(alice.sync(0).unwrap().into_iter().filter_map(|event| match event {
            Event::Message { content, .. } => Some(content),
            _ => None,
        }));
        assert!(began.elapsed() < Duration::from_millis(6000), "no echo at once");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(answers, [Content::Yo]);
    assert!(yos_until_quiet(&mut alice, Duration::from_millis(1100)).is_empty(), "one answer only");
    // Media whose blob is not there gets nothing back.
    let (photo, _) = media::seal(b"untrusted bytes", 1, 1).unwrap();
    alice.send("ECHO", Content::Photo(photo.clone())).unwrap();
    alice.send("ECHO", Content::Video { media: photo, duration_ms: 1 }).unwrap();
    assert!(yos_until_quiet(&mut alice, Duration::from_millis(1500)).is_empty());
    drop(bot);
    let log = std::fs::read_to_string(log_path).unwrap();
    assert!(!log.contains(alice.user_id()) && !log.contains(&alice.user_id()[..8]), "{log}");
    assert!(log.contains("echo answered=1"), "{log}");
    std::fs::remove_dir_all(&yo.state).unwrap();
}

#[test]
fn ping_answers_normal_yos_and_pingtest_answers_token_probes() {
    let hub = TestHub::start();
    let (ping_dir, probe_dir) = (Yo::new(&hub), Yo::new(&hub));
    let _bot = ping(&ping_dir, &[], &[]);
    let _probe_bot = probe_dir.skill("pingtest", "PINGTEST", &[], &[]);
    let mut alice = someone(&hub);
    let (user, name) = alice.whois_begin("PING").unwrap().send().unwrap();
    alice.whois_end("PING", &user, name.clone()).unwrap();
    alice.set_friend(&user, name, None).unwrap();
    alice.write_storage().unwrap();
    assert!(alice.sync(0).unwrap().is_empty(), "adding PING does not require a reply");
    assert_eq!(alice.friends()[0].user, user);
    alice.send_yo("PING").unwrap();
    assert_eq!(first_yo(&mut alice), user);
    assert_eq!(alice.contacts().len(), 1);
    assert!(alice.round_trip_probe("PING", 900).is_err());
    // The first probe waits for PINGTEST's listener to be up; the next go at once.
    let start = Instant::now();
    while alice.round_trip_probe("PINGTEST", 900).unwrap().is_none() {
        assert!(start.elapsed() < Duration::from_secs(30), "PINGTEST never answered");
    }
    let began = Instant::now();
    assert!(alice.round_trip_probe("PINGTEST", 900).unwrap().is_some());
    assert!(began.elapsed() < Duration::from_millis(900));
    assert!(alice.sync(0).unwrap().is_empty(), "PINGTEST probe is silent");
    assert_eq!(alice.contacts().len(), 1);
    assert_eq!(alice.contacts()[0].user, user);
    alice.send_yo("PING").unwrap();
    assert_eq!(yos_until_quiet(&mut alice, Duration::from_millis(800)).len(), 1);
    let _ = std::fs::remove_dir_all(&ping_dir.state);
    let _ = std::fs::remove_dir_all(&probe_dir.state);
}

/// PING sends a photo back as the same photo, a video as the same video (same bytes, size and length, sealed anew),
/// one each, and a Yo as a Yo; a photo whose blob is gone gets nothing.
#[test]
fn ping_echoes_a_photo_and_a_video_as_they_came() {
    let hub = TestHub::start();
    let yo = Yo::new(&hub);
    let _bot = ping(&yo, &[], &[]);
    let mut alice = someone(&hub);
    alice.send_yo("PING").unwrap();
    let ping_id = first_yo(&mut alice);

    let photo_bytes = picture_of(200_000);
    let fetches = alice.media_fetches("PING").unwrap();
    let photo = uploaded(&mut alice, &photo_bytes, fetches);
    alice.send("PING", Content::Photo(photo.clone())).unwrap();
    let got = echoed(&mut alice, &ping_id, None, Duration::from_secs(2));
    assert_eq!(got.len(), 1, "one photo back");
    let (Content::Photo(back), Some(bytes)) = &got[0] else { panic!("not a photo back") };
    assert_eq!(bytes, &photo_bytes);
    assert_eq!((back.width, back.height), (photo.width, photo.height));
    assert_ne!(back.id, photo.id, "uploaded anew");

    std::thread::sleep(Duration::from_millis(1100)); // a token back in its bucket
    let video_bytes = picture_of(300_000);
    let video = uploaded(&mut alice, &video_bytes, fetches);
    alice.send("PING", Content::Video { media: video, duration_ms: 4321 }).unwrap();
    let got = echoed(&mut alice, &ping_id, None, Duration::from_secs(2));
    assert_eq!(got.len(), 1, "one video back");
    let (Content::Video { duration_ms: 4321, .. }, Some(bytes)) = &got[0] else { panic!("not the video back") };
    assert_eq!(bytes, &video_bytes);

    std::thread::sleep(Duration::from_millis(1100));
    let (gone, _) = media::seal(b"never uploaded", 1, 1).unwrap();
    alice.send("PING", Content::Photo(gone)).unwrap();
    assert!(echoes(&mut alice, &ping_id, None, Duration::from_secs(2)).is_empty(), "nothing for a missing blob");
    let _ = std::fs::remove_dir_all(&yo.state);
}

/// In a group, PING sends each Yo, photo or video back into the group once, a group once per cooldown, and never
/// what a `--quiet-for` user (another bot) sent.
#[test]
fn ping_echoes_into_a_group_once_and_never_to_quiet_bots() {
    let hub = TestHub::start();
    let yo = Yo::new(&hub);
    let (mut alice, mut leo) = (someone(&hub), someone(&hub));
    let quiet = leo.username().unwrap().to_lowercase();
    let _bot = ping(&yo, &["--quiet-for", &quiet], &[("PING_GROUP_COOLDOWN_MS", "2000")]);
    alice.send_yo("PING").unwrap();
    let ping_id = first_yo(&mut alice);
    let (group, _) = alice.create_group("Echo", &[ping_id.clone(), leo.user_id().to_string()]).unwrap();
    let _ = leo.sync(0).unwrap();
    leo.acknowledge_events();

    // Two Yos at once: one Yo back into the group (the second within the cooldown).
    alice.send_group(&group, Content::Yo).unwrap();
    alice.send_group(&group, Content::Yo).unwrap();
    let got = echoed(&mut alice, &ping_id, Some(&group), Duration::from_millis(1500));
    assert_eq!(got, [(Content::Yo, None)]);

    std::thread::sleep(Duration::from_millis(1000)); // the cooldown over
    let photo_bytes = picture_of(50_000);
    let fetches = alice.group_media_fetches(&group).unwrap();
    let photo = uploaded(&mut alice, &photo_bytes, fetches);
    alice.send_group(&group, Content::Photo(photo)).unwrap();
    let got = echoed(&mut alice, &ping_id, Some(&group), Duration::from_secs(2));
    assert_eq!(got.len(), 1, "one photo back into the group");
    assert!(matches!(&got[0], (Content::Photo(_), Some(bytes)) if *bytes == photo_bytes));
    // Leo, a member too, got it from PING as well.
    let got = echoed(&mut leo, &ping_id, Some(&group), Duration::from_secs(3));
    assert!(got.iter().any(|(c, _)| matches!(c, Content::Photo(_))));

    // The quiet-for bot is never answered, after the cooldown too.
    std::thread::sleep(Duration::from_millis(2100));
    leo.send_group(&group, Content::Yo).unwrap();
    let video = uploaded(&mut leo, &photo_bytes, fetches);
    leo.send_group(&group, Content::Video { media: video, duration_ms: 1 }).unwrap();
    assert!(echoes(&mut alice, &ping_id, Some(&group), Duration::from_secs(2)).is_empty(), "answered a quiet bot");
    let _ = std::fs::remove_dir_all(&yo.state);
}

/// The same YoGPT through five hub restarts, with traffic before and during each outage.
#[test]
#[ignore = "ten-minute hub restart soak"]
fn yogpt_keeps_answering_through_repeated_hub_restarts() {
    let dir = state_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let mut config = yo_hub::config::Config::for_tests();
    config.db_path = dir.join("hub.sqlite").to_str().unwrap().to_owned();
    let mut hub = TestHub::start_with(config.clone(), "127.0.0.1:0");
    let yo = Yo::new(&hub);
    let mut alice = someone(&hub);
    let mut bot = yogpt(&yo, &[], &[]);
    let me = round_trip(&mut alice, "YOGPT").remove(0);
    for cycle in 1..=5 {
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            assert!(round_trip(&mut alice, "YOGPT").iter().all(|user| *user == me));
        }
        let addr = hub.addr.to_string();
        hub.stop();
        alice.send_yo(&me).unwrap();
        assert!(alice.waiting() > 0, "outage {cycle}: send was not queued");
        std::thread::sleep(Duration::from_secs(5));
        hub = TestHub::start_with(config.clone(), &addr);
        let answers = yos_until_quiet(&mut alice, Duration::from_secs(20));
        assert!((1..=5).contains(&answers.len()), "outage {cycle}: got {} replies", answers.len());
        assert!(answers.iter().all(|user| *user == me));
        assert!(bot.running());
    }
    assert!(hub.hub.audit().unwrap().is_empty());
    drop(bot);
    hub.stop();
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(&yo.state);
}
